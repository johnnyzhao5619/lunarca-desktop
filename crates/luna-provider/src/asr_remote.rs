//! 远程批式转写 Provider（ADR-036 ①②）。
//!
//! 把「上传 → 建任务 → 轮询 → 取结果」这条服务端流程包成一个普通的 [`AIProvider`]，
//! 于是它与本机 whisper 在 Router 眼里同构：同一个 `Capability::Transcribe`、
//! 同一个 `ResultPayload::Segments`、同一套取消语义。
//!
//! # 三条定死的东西
//!
//! 1. **`location: Cloud`**。严格本地模式按 `location` 过滤（既有机制），标错就等于
//!    在"仅本地"下把用户的录音发出去——本模块最严重的一种失败，且不会有任何报错。
//! 2. **打的是登录的那台 Lunarca 服务器**：`/inference/v1/audio/uploads` 是 Lunarca 原生
//!    API，只有 Lunarca 服务端有。
//! 3. **不重传**：[`AudioRef::Uploaded`] 直接建任务，跳过上传。重试一次转写、或应用
//!    重启后重新挂上进行中的任务，都不该让用户再传一遍几百 MB。
//!
//! # 为什么不复用 `luna-cloud` 的客户端
//!
//! 那个 crate 是 Tauri 插件（带命令面与 ACL），而 Provider 活在 `luna-provider`——
//! 一个零 Tauri 依赖的库。凭据经 [`BearerSource`] 端口注入，装配层把它接到 luna-cloud
//! 的会话上。**刻意不让本模块自己碰钥匙链**：那样就有了第二条读凭据的路径，而
//! 「未登录零云网络行为」的机器断言只盯着 luna-cloud 的命令面。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_stream::stream;
use async_trait::async_trait;
use luna_core::{
    AIProvider, AudioRef, CancelToken, Capability, CapabilitySet, ExecutorInfo, ExecutorKind, Job,
    JobError, JobEvent, JobEventStream, JobKind, JobResult, ModelId, ProviderDescriptor,
    ProviderHealth, ProviderId, ProviderLocation, ResultPayload, Segment, Timestamp, Timings,
    Usage, UsageEstimate,
};
use luna_model_fetch::pinned_tls::PinnedClient;
use serde::Deserialize;

/// 本 Provider 的稳定 id。
pub const REMOTE_ASR_PROVIDER_ID: &str = "remote-asr";

/// 上传分片大小。服务端会给建议值，但客户端得有个默认——首片发出去之前还不知道建议。
const DEFAULT_CHUNK_BYTES: usize = 8 * 1024 * 1024;

/// 轮询间隔。够密到进度不显得卡住，够疏到不把服务端问烦。
const POLL_INTERVAL: Duration = Duration::from_millis(1500);

/// 单次转写的整体等待上限。
const MAX_WAIT: Duration = Duration::from_secs(60 * 60);

/// 轮询次数上限——**由上面两个常量推出来**，不手写。
///
/// 手写的那版是 `4 * 60 * 40 // ≈ 1 小时`，实际是 4 小时：注释与常量各说各话，
/// 而没有任何东西会发现。凡是「两个数必须一致」的地方，就让其中一个算出来。
const MAX_POLLS: u32 = (MAX_WAIT.as_millis() / POLL_INTERVAL.as_millis()) as u32;

/// 取消检查的节奏（事件流等帧期间）。
///
/// 与轮询间隔同量级：取消的可见延迟不该因为换了传输方式而变差。
const CANCEL_POLL: Duration = Duration::from_millis(500);

/// 轮询期间容忍的**连续**失败次数。
///
/// 单次网络抖动不该杀掉任务：服务端那边还在跑、还在计费，而客户端已经报错退出，
/// 用户既拿不到结果也不知道自己仍在被扣额度。只有连续失败到这个次数（≈ 30 秒）
/// 才认定链路真的断了。
const MAX_CONSECUTIVE_POLL_FAILURES: u32 = 20;

/// 会话凭据来源。
///
/// 端口而非直接读钥匙链——见模块头第三节。返回 `None` = 未登录，本 Provider 随即
/// 以 `Unauthorized` 失败而**不发任何请求**（零云网络行为不变量）。
#[async_trait]
pub trait BearerSource: Send + Sync {
    async fn bearer(&self) -> Option<String>;

    /// 收到 401 后重取一次（实现方内部完成刷新）。`None` = 确实需要重新登录。
    ///
    /// 存在的理由：**一次上传可能跨过 access token 的有效期**。几百 MB 的会议录音
    /// 传十几分钟，而 access token 是分钟级的——不刷新的话，用户传了十几分钟然后
    /// 眼睁睁看着整个上传作废。`luna-cloud` 的 `authed` 早就是「401 → 刷新 → 重试一次」
    /// 这个形状，这里只是把同一条纪律带到 Provider 侧。
    ///
    /// # 为什么要把 `rejected` 传进来
    ///
    /// 它是**刚刚挨了 401 的那一枚**，也是实现方分辨「真过期」与「别人已经换好了」的
    /// 唯一依据：几路 Provider 常常同时撞上同一次过期（一次长回答 + 一批嵌入 + 一份上传），
    /// 谁先抢到续期锁谁轮换，其余几路醒来时钥匙链里已是新 token——此时再轮换一次纯属
    /// 多一个往返，而且每一轮都在消耗服务端的一次性 refresh。不传的话实现方只知道
    /// 「有人 401 了」，无从判断这枚是不是已经被换掉的旧货，只能盲目串行轮换 N 次
    /// （TDD-14 §3.3.1 ⑩ 记录的偏差，本参数即修法）。
    ///
    /// 参数是**必填**而不是 `Option`：漏传要在编译期就红，而不是退化成那 N-1 次往返
    /// ——后者在测试里完全看不出来，只有对着真服务端数请求才发现。
    async fn refresh(&self, rejected: &str) -> Option<String>;
}

pub struct RemoteAsrConfig {
    /// Lunarca 服务端基址（登录的那台服务器）。
    pub api_base: String,
    pub bearer: Arc<dyn BearerSource>,
    /// 外发闸（严格本地随时可掐断这条腿）。见 [`luna_core::OutboundGate`]。
    ///
    /// 批式看起来「一次任务就结束了」，但上传一份会议录音是几分钟的持续外发——
    /// 用户中途按下「仅本地」时，同样必须当场停，而不是等这份传完。
    pub gate: Arc<dyn luna_core::OutboundGate>,
}

pub struct RemoteAsrProvider {
    config: RemoteAsrConfig,
    /// 自签部署钉住的根证书在这里生效（ADR-153）。
    http: PinnedClient,
}

impl RemoteAsrProvider {
    #[must_use]
    pub fn new(config: RemoteAsrConfig) -> Self {
        Self {
            config,
            http: PinnedClient::default(),
        }
    }
}

#[async_trait]
impl AIProvider for RemoteAsrProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(REMOTE_ASR_PROVIDER_ID)
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
            display_name: "服务端转写".to_owned(),
            // 用哪个模型由服务端决定，会话建立前不知道；它随 `executed_by` 回报。
            model: None,
            // 见模块头第 1 条：标错这一项，严格本地模式就拦不住它。
            location: ProviderLocation::Cloud,
            license_summary: None,
        }
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet {
            capabilities: [Capability::Transcribe].into_iter().collect(),
            // 批式通道不产生增量——声明 streaming 会让 UI 等一个永不到来的 partial。
            streaming: false,
            location: ProviderLocation::Cloud,
            explicit_only: false,
        }
    }

    async fn health(&self) -> ProviderHealth {
        // 不主动探活：探一次就是一次云网络行为（ADR-037 不变量），而真实可用性
        // 在首个任务上就会暴露，且那时是用户显式发起的。
        ProviderHealth::ready()
    }

    async fn execute(&self, job: Job, cancel: CancelToken) -> JobEventStream {
        let JobKind::Transcribe {
            audio, lang_hint, ..
        } = job.kind.clone()
        else {
            return once_failed(JobError::InvalidInput("非转写 Job".into()));
        };

        let source = match audio_source(&audio) {
            Ok(v) => v,
            Err(e) => return once_failed(e),
        };
        // 严格本地：一个请求都不发。放在取凭据**之前**（先探服务端再发现不该发，
        // 已经是一次可观测的云网络行为了）。
        if !self.config.gate.allows_outbound() {
            return once_failed(JobError::StrictLocalBlocked {
                capability: luna_core::Capability::Transcribe,
            });
        }
        let Some(mut token) = self.config.bearer.bearer().await else {
            // 未登录时**一个字节都不发**（ADR-037 零云网络行为不变量）。
            // 用 `Unauthorized` 而非 `Upstream`：前者在 UI 上是「去登录」这个可执行动作，
            // 后者只会显示「上游出错了」让用户无从下手，且会被当成可重试反复退避。
            return once_failed(unauthorized());
        };

        let http = self.http.for_url(&self.config.api_base);
        let bearer = Arc::clone(&self.config.bearer);
        let gate = Arc::clone(&self.config.gate);
        let gate_for_stream = Arc::clone(&gate);
        let base = self.config.api_base.trim_end_matches('/').to_owned();
        let job_id = job.id;
        // 档位由**本机优先级**映射而来，映射只有一份（`QueueTier::from_priority`）。
        // 在这里取而不是让调用方传：`Job.priority` 已经承载了「用户在不在等」这件事
        // （TDD-01 §5.2），再开一个参数就等于让同一个事实有两个来源，而它们迟早分叉。
        let tier = luna_core::QueueTier::from_priority(job.priority);
        let lang = lang_hint.map(|l| l.as_str().to_owned());

        Box::pin(stream! {
            yield JobEvent::Queued;
            // 三段链路共用同一个「该停了吗」（见 `Interrupt`）。
            let interrupt = Interrupt { cancel: &cancel, gate: gate_for_stream.as_ref() };

            let upload_id = match source {
                Source::Uploaded(id) => id,
                Source::Pcm(pcm) => {
                    yield JobEvent::Started;
                    // PCM 已在内存里，不必分片流式传——批量档的窗口通常是分钟级片段。
                    // 超大 PCM 本就不该走这条路（那是文件导入，走 `Source::File`）。
                    let wav = pcm_to_wav(&pcm);
                    let session = UploadSession {
                        http: &http,
                        base: &base,
                        bearer: bearer.as_ref(),
                        interrupt: Interrupt { cancel: &cancel, gate: gate.as_ref() },
                    };
                    match upload_bytes(&session, &mut token, wav).await {
                        Ok(id) => id,
                        Err(JobError::Cancelled) => { yield JobEvent::Cancelled; return; }
                        Err(e) => { yield JobEvent::Failed(e); return; }
                    }
                }
                Source::File(path) => {
                    yield JobEvent::Started;
                    // 进度**实时**发，不是攒到最后一起发：上传占整条链路的绝大部分
                    // 时间（几百 MB 的会议录音 vs 几秒的解码），攒着发等于没有进度——
                    // 用户看到「开始了」然后长时间毫无动静，与卡死完全同形。
                    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<f32>();
                    let task = {
                        let (http, base, cancel) = (http.clone(), base.clone(), cancel.clone());
                        let bearer = Arc::clone(&bearer);
                        let gate = Arc::clone(&gate);
                        // token 搬进去、再搬回来：上传途中可能刷新过，后续的建任务
                        // 与轮询必须用**刷新后的那个**，否则每一步都要各自再刷一次。
                        let mut tok = token.clone();
                        tokio::spawn(async move {
                            let session = UploadSession {
                                http: &http,
                                base: &base,
                                bearer: bearer.as_ref(),
                                interrupt: Interrupt { cancel: &cancel, gate: gate.as_ref() },
                            };
                            let r = upload_file(&session, &mut tok, &path, tx).await;
                            (r, tok)
                        })
                    };
                    // `upload_file` 持有 tx，它一返回 channel 就关闭 → 循环自然结束。
                    while let Some(p) = rx.recv().await {
                        yield JobEvent::Progress(p);
                    }
                    match task.await {
                        Ok((Ok(id), refreshed)) => { token = refreshed; id }
                        // 上传期间取消是**取消**，不是失败。真机坐实（2026-08-07）：
                        // 在上传窗口点取消，客户端此前收到 `Failed(Cancelled)`——用户点了
                        // 取消却看到一个报错，在界面上是完全不同的两件事（会让人以为
                        // 出了问题而去重试、去报 bug）。轮询窗口取消一直是对的，只有
                        // 上传这条路径漏了这层映射。
                        Ok((Err(JobError::Cancelled), _)) => { yield JobEvent::Cancelled; return; }
                        Ok((Err(e), _)) => { yield JobEvent::Failed(e); return; }
                        Err(join) => {
                            yield JobEvent::Failed(JobError::Upstream {
                                provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
                                code: None,
                                message: format!("上传任务异常终止：{join}"),
                            });
                            return;
                        }
                    }
                }
            };

            // **幂等键（`job_id`）与体里的 `upload_id` 是两个生命周期，别让它们分叉。**
            //
            // 上传发生在本函数内，故「同一个 Job 被执行两次」会产生**新的 upload_id**，
            // 而键不变 → 服务端判成同键不同体 → 422。今天没有路径会那样：队列的回落换的是
            // 下一个 Provider，`asr_failover` 是实时通道的两条腿，`send_authed` 的 401 重发
            // 用的是同一个闭包（同一个 upload_id），三条都不会重跑本函数。
            //
            // 若将来给本 Provider 加「同一条腿重试」，先决定要哪一种失败：保持现状是 422
            // （不重复扣费，但错误指向客户端）；把 upload_id 并进键则变成一次全新请求
            // （错误消失，但第一次若已建出任务就是**重复扣费**）。前者是更安全的那一侧，
            // 这也是它维持原样的理由。
            let created = match create_job(
                &http, &base, bearer.as_ref(), &mut token, &upload_id, lang.as_deref(), tier,
                job_id,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => { yield JobEvent::Failed(e); return; }
            };
            let remote_job = created.job_id;
            // 建任务的响应里就带排位——先发出去，别等第一帧 SSE。
            //
            // 订阅本身要一次往返，而批量档的第一帧排位刷新可能在几分钟之后（它只在
            // 前面有任务出队时才发）。少了这一句，用户提交完看到的是一个什么都不说的
            // 转圈，而服务端明明已经告诉过我们他排第几。
            if let Some(queue) = created.queue {
                yield JobEvent::RemoteQueued(queue.into_status());
            }

            // 先走事件流：终态即时可知、**执行者可见**（PRD-17-033）、**排位实时刷新**
            // （ADR-049 ⑧）。SSE 不通就回落轮询——反代缓冲 SSE 是常见部署问题，唯一
            // 依赖它会让转写在那些环境里彻底不可用。
            let mut terminal: Option<PhaseDto> = None;
            let mut stopped: Option<JobError> = None;
            {
                use futures::StreamExt as _;
                let mut watched = std::pin::pin!(watch_events(
                    &http, &base, &token, &remote_job,
                    interrupt
                ));
                while let Some(event) = watched.next().await {
                    match event {
                        // 如实告诉用户是谁跑的（PRD-17-033）。静默替换是信任事故：
                        // 用户以为在用组织的服务端，实际被降级到了第三方，而界面上毫无差别。
                        WatchEvent::Executor(who) => yield JobEvent::ExecutedBy(who),
                        WatchEvent::Queued(status) => yield JobEvent::RemoteQueued(status),
                        WatchEvent::Terminal(phase) => { terminal = Some(phase); break; }
                        WatchEvent::Stop(reason) => { stopped = Some(reason); break; }
                    }
                }
            }
            if let Some(reason) = stopped {
                // 取消与「仅本地」都要 DELETE 掉服务端任务，只是给用户的结论不同；
                // 连接层错误则静默回落轮询（转写照样能完成，只是少了即时性）。
                match reason {
                    JobError::Cancelled => {
                        let _ = cancel_job(&http, &base, &token, &remote_job).await;
                        yield JobEvent::Cancelled;
                        return;
                    }
                    JobError::StrictLocalBlocked { .. } => {
                        let _ = cancel_job(&http, &base, &token, &remote_job).await;
                        yield JobEvent::Failed(reason);
                        return;
                    }
                    _ => {}
                }
            }
            if let Some(phase) = terminal {
                match phase.state.as_str() {
                    "finished" => {
                        // 结果本身仍走 GET 取（**取走即删**的语义在那一侧）。
                        match poll_job(&http, &base, bearer.as_ref(), &mut token, &remote_job).await
                        {
                            // **GET 说了算，SSE 不算**。这一帧只是「服务端广播了终态」，
                            // 而 GET 那一侧才是「任务行 + 结果缓存」的原子读。两者可以短暂
                            // 不一致（服务端此前是先发帧、后落行），此刻 GET 会如实回
                            // `state:"running", segments:null`——**那不是一个空结果，
                            // 是一个还没就绪的状态**。
                            //
                            // 处置是**回落到下面的轮询循环**，不在这里判失败也不判空：
                            // 轮询本来就存在、本来就是原子读，且终态判定只有它那一份。
                            // 代价是多发一次 GET（循环开头立刻再问一次），换来的是
                            // 「终态怎么算」不会在本文件里出现第二份实现。
                            Ok(status) if status.state != "finished" => {}
                            Ok(status) => {
                                // `None` ≠ `Some([])`，见 `JobError::ResultUnavailable`。
                                // 服务端刻意把「结果不在」回成缺席而不是空数组；在这里
                                // `unwrap_or_default()` 就是把它刚分开的两件事又合回去，
                                // 而合回去的那一份会被写进 Vault。
                                let Some(segments) = status.segments else {
                                    yield JobEvent::Failed(JobError::ResultUnavailable {
                                        provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
                                    });
                                    return;
                                };
                                yield JobEvent::Progress(1.0);
                                yield JobEvent::Finished(JobResult {
                                    job_id,
                                    provider_id: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
                                    model_id: Some(ModelId::from(REMOTE_ASR_PROVIDER_ID)),
                                    payload: ResultPayload::Segments(segments),
                                    usage: Usage::default(),
                                    timings: Timings::default(),
                                    output_truncated: None,
                                    finished_at: Timestamp::ZERO,
                                });
                                return;
                            }
                            Err(e) => { yield JobEvent::Failed(e); return; }
                        }
                    }
                    "cancelled" => { yield JobEvent::Cancelled; return; }
                    // `expired` 走这里：`map_terminal_error` 把它翻成 `QueueExpired`
                    // （不可自动重试，但界面给显式重试入口，见那条错误的文档）。
                    // 服务端在该终态下**没有计费**，也**没有删音频**。
                    other => {
                        yield JobEvent::Failed(map_terminal_error(
                            phase.error_code.as_deref().or(Some(other)),
                        ));
                        return;
                    }
                }
            }

            // 轮询到终态（事件流的回落）。取消 → DELETE 掉服务端任务。
            let mut polls = 0u32;
            let mut consecutive_failures = 0u32;
            loop {
                // 取消，或用户切到了「仅本地」——两者都要 DELETE 掉服务端任务
                // （它会停止解码并删掉临时音频），只是给用户的结论不同。
                if let Some(stop) = interrupt.reason() {
                    let _ = cancel_job(&http, &base, &token, &remote_job).await;
                    yield if matches!(stop, JobError::Cancelled) {
                        JobEvent::Cancelled
                    } else {
                        JobEvent::Failed(stop)
                    };
                    return;
                }
                match poll_job(&http, &base, bearer.as_ref(), &mut token, &remote_job).await {
                    Ok(status) => { consecutive_failures = 0; match status.state.as_str() {
                        "finished" => {
                            // 与 SSE 那条路同一判据：**缺席不是空**。
                            // `None` = 结果取不回来（已被取走 / 已过期），
                            // `Some([])` = 真的转写出来是空的（纯静音之类，服务端对它另有
                            // 兜底）——后者是合法结果，照常 `Finished`。
                            let Some(segments) = status.segments else {
                                yield JobEvent::Failed(JobError::ResultUnavailable {
                                    provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
                                });
                                return;
                            };
                            yield JobEvent::Progress(1.0);
                            yield JobEvent::Finished(JobResult {
                                job_id,
                                provider_id: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
                                model_id: Some(ModelId::from(REMOTE_ASR_PROVIDER_ID)),
                                payload: ResultPayload::Segments(segments),
                                usage: Usage::default(),
                                timings: Timings::default(),
                                output_truncated: None,
                                finished_at: Timestamp::ZERO,
                            });
                            return;
                        }
                        "cancelled" => { yield JobEvent::Cancelled; return; }
                        // 判据取自 `terminal_state`，不再手抄一份字符串列表：ADR-049
                        // 加了 `expired`，而漏改这一处的症状是「过期的任务在轮询这条路上
                        // 一直转圈」——直到轮询预算耗尽才报一句超时。
                        other if terminal_state(other) => {
                            yield JobEvent::Failed(map_terminal_error(
                                status.error_code.as_deref().or(Some(other)),
                            ));
                            return;
                        }
                        // queued / running：继续等。
                        // 排位也要在这条路上报出来（ADR-049 ⑧）：反代缓冲 SSE 是常见
                        // 部署问题，那些环境里客户端只有轮询。只在事件流上报的话，
                        // 同一台服务端在两种部署下会显示不同的东西。
                        _ => {
                            if let Some(queue) = status.queue {
                                yield JobEvent::RemoteQueued(queue.into_status());
                            }
                        }
                    } },
                    Err(e) => {
                        // 见 `MAX_CONSECUTIVE_POLL_FAILURES`：服务端任务还在跑，
                        // 一次抖动就退出等于让用户白白被计费还拿不到结果。
                        consecutive_failures += 1;
                        if consecutive_failures >= MAX_CONSECUTIVE_POLL_FAILURES {
                            yield JobEvent::Failed(e);
                            return;
                        }
                    }
                }

                polls += 1;
                if polls >= MAX_POLLS {
                    // 报「超时」而不是继续等：一个永远 running 的任务在界面上
                    // 与"正在转写"完全同形，用户会一直等下去。
                    yield JobEvent::Failed(JobError::Timeout);
                    return;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
    }

    fn estimate(&self, _job: &Job) -> Option<UsageEstimate> {
        None // 服务端权威计费；客户端不猜（TDD-14 §3）。
    }
}

/// 本 Provider 接受的音频来源。
#[derive(Debug)]
enum Source {
    /// 本地文件——需要先上传。
    File(PathBuf),
    /// 内存 PCM——**加 WAV 容器头后上传**。
    ///
    /// 必须支持这一档，否则本 Provider 在真实链路上根本用不上：批量转写流水线
    /// （导入 / 二次转写）一律先本地解码成 16k mono PCM 再提交 `AudioRef::Pcm`
    /// （见 `pipeline::transcribe_pcm`），从不给文件路径。只收 `File` 等于做了一个
    /// 谁都不会走的 Provider。
    Pcm(luna_core::PcmBuffer),
    /// 服务端已有的上传——直接建任务（不重传）。
    Uploaded(String),
}

/// `AudioRef` → 来源。拒绝的两种各给出**能定位到问题**的说明。
fn audio_source(audio: &AudioRef) -> Result<Source, JobError> {
    match audio {
        AudioRef::File(path) => Ok(Source::File(path.clone())),
        AudioRef::Uploaded(id) => Ok(Source::Uploaded(id.as_str().to_owned())),
        // 实时流走的是另一条通道（ADR-038 的双向流式），不是这里。
        AudioRef::Stream(_) => Err(JobError::InvalidInput(
            "实时流不走批式转写通道（应路由到实时转写 Provider）".into(),
        )),
        // 裸 PCM 没有容器头，服务端交给 whisper 只会报一个语焉不详的格式错——
        // 但那不是拒绝它的理由：加个 44 字节的 WAV 头就行，无损、成本可忽略。
        AudioRef::Pcm(pcm) => Ok(Source::Pcm(pcm.clone())),
    }
}

/// 发一次请求；401 时刷新凭据后**只重试一次**。
///
/// 与 `luna_cloud::CloudClient::authed` 同一形状：刷新后仍被拒（如设备刚被撤销的
/// 竞态）就收敛到「需要重新登录」，不无限重试。
async fn send_authed<F>(
    bearer: &dyn BearerSource,
    token: &mut String,
    build: F,
) -> Result<reqwest::Response, JobError>
where
    F: Fn(&str) -> reqwest::RequestBuilder,
{
    let response = build(token).send().await.map_err(net_err)?;
    if response.status() != reqwest::StatusCode::UNAUTHORIZED {
        return check_status(response).await;
    }
    // 传的是**刚被拒的这一枚**（而不是回头再读一次凭据）：实现方据此分辨真过期与
    // 「等锁期间别人已经换好」，见 `BearerSource::refresh`。
    let Some(fresh) = bearer.refresh(token).await else {
        return Err(unauthorized());
    };
    *token = fresh;
    check_status(build(token).send().await.map_err(net_err)?).await
}

/// 「需要登录」的统一构造（刷新令牌也换不出可用会话时的收场）。
///
/// **必须是 [`JobError::Unauthorized`] 而不是带 `UNAUTHORIZED` 码的
/// [`JobError::Upstream`]**——后者是这个函数长期以来的写法，代价有两条，都发生在用户
/// 那一侧：
///
/// - `Upstream` 是**可重试**的（[`JobError::is_retriable`]），于是 JobQueue 会反复重发
///   一个凭据没换就必然再 401 的请求；
/// - 前端读的是**变体**投影出的稳定码（`JobErrorCodeDto`），塞在 `Upstream.code` 里的
///   那个字符串它根本看不到 —— 「去登录」的深链因此退化成一句「上游出错」。
///
/// 本文件另外三处（未登录短路、服务端 `unauthorized` 阶段码、401/403）早已用对了变体，
/// 只有这一处漏网。现在四处都经由本函数构造。
fn unauthorized() -> JobError {
    JobError::Unauthorized {
        provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
    }
}

#[derive(Deserialize)]
struct CreateUploadDto {
    upload_id: String,
    suggested_chunk_bytes: Option<u64>,
}

#[derive(Deserialize)]
struct CreateJobDto {
    job_id: String,
    /// 入队后的排位（ADR-049 ⑧）。仅当服务端把它排进了队列时在场。
    ///
    /// **同步响应里就带**，客户端因此不必「提交完再去查一次」——那一查与入队之间
    /// 队列已经变了，而用户看到的第一个数字恰恰是他印象最深的那个。
    #[serde(default)]
    queue: Option<QueueDto>,
}

/// 服务端队列排位的线上形状（建任务响应、轮询响应与 `queued` SSE 帧共用同一组字段）。
#[derive(Deserialize)]
struct QueueDto {
    position: u32,
    #[serde(default)]
    tier: Option<String>,
    #[serde(default)]
    eta_range: Option<EtaRangeDto>,
}

#[derive(Deserialize)]
struct EtaRangeDto {
    min_secs: u32,
    max_secs: u32,
}

/// `queued` SSE 帧：形状与 [`QueueDto`] 平铺在一起（服务端在事件里不套一层 `queue`）。
#[derive(Deserialize)]
struct QueuedFrameDto {
    position: u32,
    #[serde(default)]
    tier: Option<String>,
    #[serde(default)]
    eta_range: Option<EtaRangeDto>,
}

impl QueuedFrameDto {
    fn into_status(self) -> luna_core::RemoteQueueStatus {
        status_of(self.position, self.tier.as_deref(), self.eta_range)
    }
}

impl QueueDto {
    fn into_status(self) -> luna_core::RemoteQueueStatus {
        status_of(self.position, self.tier.as_deref(), self.eta_range)
    }
}

/// 三处（建任务响应 / 轮询响应 / SSE 帧）共用同一份换算。
///
/// 各写一遍的代价很具体：其中一处忘了把 `eta_range` 的缺席保持成 `None`，
/// 就会在界面上编出一个「预计 0–0 分钟」，而那比不显示更糟。
fn status_of(
    position: u32,
    tier: Option<&str>,
    eta: Option<EtaRangeDto>,
) -> luna_core::RemoteQueueStatus {
    luna_core::RemoteQueueStatus {
        position,
        tier: tier.map_or(luna_core::QueueTier::Standard, luna_core::QueueTier::parse),
        eta: eta.map(|e| luna_core::EtaRange {
            min_secs: e.min_secs,
            max_secs: e.max_secs,
        }),
    }
}

/// 服务端阶段帧（`/events` 的 SSE 载荷）。
///
/// **没有百分比字段，这是刻意的**：上游（whisper.cpp server）给不出解码进度，
/// 服务端如实只报阶段而不编一个数。客户端相应地在解码期间显示"转写中"而不是一个
/// 会卡在 80% 十分钟的进度条。
#[derive(Deserialize)]
struct PhaseDto {
    state: String,
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    executed_by: Option<ExecutedByDto>,
}

/// 谁跑的这次转写（PRD-17-033 降级执行者可见）。
#[derive(Deserialize, Debug)]
struct ExecutedByDto {
    upstream_name: String,
    kind: String,
    model: String,
}

#[derive(Deserialize)]
struct StatusDto {
    state: String,
    error_code: Option<String>,
    /// 转写结果。**`None` 与 `Some(vec![])` 在协议上是两件事，不许合并。**
    ///
    /// - `None`（字段缺席 / `null`）= **结果不在**：已被取走（那一侧是「取走即删」）、
    ///   已过期，或行与缓存尚未一致。服务端刻意回缺席而不是空数组，原话是「过期的当作
    ///   不存在——空数组在客户端表现为『转写出来是空的』，与『结果已过期』完全同形」。
    /// - `Some(vec![])` = **转写成功、内容为空**（整段没人说话之类）。合法结果。
    ///
    /// 用 `unwrap_or_default()` 读它，就是把服务端刻意分开的两件事在客户端合回一处；
    /// 合回去的那一份会以 `Finished(Segments([]))` 写进 Vault，而 Vault 是唯一事实源。
    /// 现网 3/3 复现的空转写块就是这么来的（见 [`JobError::ResultUnavailable`]）。
    segments: Option<Vec<Segment>>,
    /// 排位——**轮询也拿得到**（ADR-049 ⑧）。
    ///
    /// 存在的理由：反代缓冲 SSE 是常见部署问题，那些环境里客户端只有轮询这一条路。
    /// 排位只在事件流上给的话，同一台服务端在两种部署下会显示不同的东西。
    #[serde(default)]
    queue: Option<QueueDto>,
}

/// 交错 f32 PCM → 16-bit WAV 字节。
///
/// **16-bit 而不是 32-bit float WAV**：whisper.cpp 收 f32 WAV 也能解，但体积翻倍，
/// 而上传是这条链路上最贵的一段。裁剪到 [-1, 1] 再定点化是无损转换的标准做法——
/// 不裁剪的话，超幅样本回绕成反相噪声，表现为"转写突然听不懂了"。
fn pcm_to_wav(pcm: &luna_core::PcmBuffer) -> Vec<u8> {
    let channels = pcm.channels.max(1);
    let bits = 16u16;
    let byte_rate = pcm.sample_rate * u32::from(channels) * u32::from(bits / 8);
    let block_align = channels * (bits / 8);
    let data_len = (pcm.samples.len() * 2) as u32;

    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt 块长度
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&pcm.sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in &pcm.samples {
        let clamped = s.clamp(-1.0, 1.0);
        out.extend_from_slice(&((clamped * i16::MAX as f32) as i16).to_le_bytes());
    }
    out
}

/// 「这条链路现在该停下吗」——批式转写的**三段**（上传 / 事件流 / 轮询）共用。
///
/// 收成一处的理由：这两个理由的**处置完全相同**（停下、把服务端那边的东西清掉），
/// 只是给用户的结论不同。分开写必然有一天只在其中一段加了新判据——`gate` 就差点只加在
/// 上传那一段：上传完之后翻闸，客户端会继续轮询（云网络行为），而服务端还在解码用户的
/// 录音，直到它自己跑完。用户按下的是「仅本地」，不是「传完这一份就好」。
#[derive(Clone, Copy)]
struct Interrupt<'a> {
    cancel: &'a CancelToken,
    gate: &'a dyn luna_core::OutboundGate,
}

impl Interrupt<'_> {
    /// 该停下吗，以及停下的理由。
    fn reason(&self) -> Option<JobError> {
        if self.cancel.is_cancelled() {
            return Some(JobError::Cancelled);
        }
        // 上传一份会议录音是几分钟的**持续外发**，之后的轮询同样在联网。用户中途
        // 按下「仅本地」时必须当场停，而不是等这份跑完——否则隐私档显示「仅本地」，
        // 字节还在往外走。
        if !self.gate.allows_outbound() {
            return Some(JobError::StrictLocalBlocked {
                capability: luna_core::Capability::Transcribe,
            });
        }
        None
    }
}

/// 一次上传会话里**全程不变**的那些东西。
///
/// 抽出来不是为了少写几个参数，而是因为它们确实是同一层：端点、凭据来源、
/// 以及「现在该不该停」。分片循环每转一圈都要问最后一个，平铺成参数时它会被漏传到
/// 新加的路径上——`gate` 差点就是这么漏的。
struct UploadSession<'a> {
    http: &'a reqwest::Client,
    base: &'a str,
    bearer: &'a dyn BearerSource,
    interrupt: Interrupt<'a>,
}

impl UploadSession<'_> {
    fn interrupted(&self) -> Option<JobError> {
        self.interrupt.reason()
    }

    /// 告诉服务端把这份上传丢掉，不留给 6 小时 TTL 去回收。
    ///
    /// 真机坐实（2026-08-07）：不这么做的话，上传窗口中止会在服务端磁盘上留下半截
    /// 录音，库里 `state=receiving`、无对应任务，只能等过期——用户明确停了，录音却还
    /// 躺在服务器上，与 ADR-036 的「即删」姿态相悖。
    ///
    /// 尽力而为：删不掉也只是回落到 GC，不该反过来改变用户已经拿到的结论。
    async fn abandon(&self, token: &str, upload_id: &str) {
        let _ = self
            .http
            .delete(format!(
                "{}/inference/v1/audio/uploads/{upload_id}",
                self.base
            ))
            .bearer_auth(token)
            .send()
            .await;
    }

    /// 建一份上传，拿回 id 与服务端建议的分片大小。
    async fn create(
        &self,
        token: &mut String,
        total_bytes: u64,
    ) -> Result<CreateUploadDto, JobError> {
        send_authed(self.bearer, token, |tok| {
            self.http
                .post(format!("{}/inference/v1/audio/uploads", self.base))
                .bearer_auth(tok)
                .json(&serde_json::json!({ "total_bytes": total_bytes }))
        })
        .await?
        .json()
        .await
        .map_err(net_err)
    }
}

/// 上传一份已在内存里的音频（PCM 档）。与 [`upload_file`] 共用同一组端点。
async fn upload_bytes(
    session: &UploadSession<'_>,
    token: &mut String,
    bytes: Vec<u8>,
) -> Result<String, JobError> {
    let created = session.create(token, bytes.len() as u64).await?;
    let chunk = created
        .suggested_chunk_bytes
        .and_then(|v| usize::try_from(v).ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_CHUNK_BYTES);
    // 仍然分片：内存里是一整块，但服务端的体上限照样生效——一次性发完整块
    // 在长片段上会 413（与文件路径同一个上限）。
    for piece in bytes.chunks(chunk) {
        if let Some(stop) = session.interrupted() {
            session.abandon(token, &created.upload_id).await;
            return Err(stop);
        }
        let piece = piece.to_vec();
        send_authed(session.bearer, token, |tok| {
            session
                .http
                .patch(format!(
                    "{}/inference/v1/audio/uploads/{}",
                    session.base, created.upload_id
                ))
                .bearer_auth(tok)
                .body(piece.clone())
        })
        .await?;
    }
    Ok(created.upload_id)
}

async fn upload_file(
    session: &UploadSession<'_>,
    token: &mut String,
    path: &Path,
    progress: tokio::sync::mpsc::UnboundedSender<f32>,
) -> Result<String, JobError> {
    let bytes = tokio::fs::metadata(path)
        .await
        .map_err(|e| JobError::InvalidInput(format!("读不到音频文件：{}", e.kind())))?
        .len();
    let created = session.create(token, bytes).await?;

    let chunk = created
        .suggested_chunk_bytes
        .and_then(|v| usize::try_from(v).ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_CHUNK_BYTES);

    // 按块读、按块发，不整读进内存：一段会议录音几百 MB，整读会让内存峰值
    // 直接等于文件大小——在低配设备上这正是我们要解决的问题，不是制造的问题。
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| JobError::InvalidInput(format!("打不开音频文件：{}", e.kind())))?;
    let mut buf = vec![0u8; chunk];
    let mut sent = 0u64;
    loop {
        // 取消，或用户中途切到了「仅本地」——两者都要立刻停并把这份上传丢掉
        // （判据与清理见 `UploadSession::interrupted` / `abandon`）。
        if let Some(stop) = session.interrupted() {
            session.abandon(token, &created.upload_id).await;
            return Err(stop);
        }
        // `read` **不保证填满缓冲区**。tokio 的文件读还有一个 2 MiB 的内部缓冲上限，
        // 于是直接拿返回值当分片时，实际分片恒为 2 MiB——真机上 26 MB 的文件被切成 14 片
        // 而不是按服务端建议的 4 片（进度点数量把这件事暴露了出来）。后果是请求数成倍，
        // 每片各付一次往返，且服务端的分片建议形同虚设。
        let mut n = 0usize;
        while n < buf.len() {
            let got = file
                .read(&mut buf[n..])
                .await
                .map_err(|e| JobError::InvalidInput(format!("读音频文件失败：{}", e.kind())))?;
            if got == 0 {
                break; // EOF
            }
            n += got;
        }
        if n == 0 {
            break;
        }
        let chunk_bytes = buf[..n].to_vec();
        send_authed(session.bearer, token, |tok| {
            session
                .http
                .patch(format!(
                    "{}/inference/v1/audio/uploads/{}",
                    session.base, created.upload_id
                ))
                .bearer_auth(tok)
                .body(chunk_bytes.clone())
        })
        .await?;

        sent += n as u64;
        // 上传封顶 0.9：剩下的留给服务端转写，否则进度会先冲到 100% 再干等着。
        let ratio = if bytes == 0 {
            0.0
        } else {
            (sent as f32 / bytes as f32 * 0.9).min(0.9)
        };
        // 发送失败 = 消费者已走（任务被丢弃），不是错误，继续传完当前分片即可
        // ——上传是幂等的按序追加，半途而废反而留下一份不完整的临时文件。
        let _ = progress.send(ratio);
    }
    Ok(created.upload_id)
}

#[allow(clippy::too_many_arguments)]
async fn create_job(
    http: &reqwest::Client,
    base: &str,
    bearer: &dyn BearerSource,
    token: &mut String,
    upload_id: &str,
    lang_hint: Option<&str>,
    tier: luna_core::QueueTier,
    job_id: luna_core::JobId,
) -> Result<CreateJobDto, JobError> {
    let body = create_job_body(upload_id, lang_hint, tier);
    let dto: CreateJobDto = send_authed(bearer, token, |tok| {
        // 幂等键要盖在**闭包里**，因为 `send_authed` 会在 401 之后拿同一个闭包再发
        // 一次——那一发正是「同一次操作的重发」，而第一发可能已经建出任务并预留了额度。
        // 本端点的重放语义还更强一档：它的结果就是 `{job_id, state}`（纯元数据），
        // 服务端会把**原任务**原样回给我们，于是重发不会留下第二个任务。
        crate::idempotency::tag(
            http.post(format!("{base}/inference/v1/transcriptions"))
                .bearer_auth(tok)
                .json(&body),
            job_id,
        )
    })
    .await?
    .json()
    .await
    .map_err(net_err)?;
    Ok(dto)
}

/// 建任务的请求体。抽成纯函数是为了让「发出去的到底是什么」可被单测钉住——
/// 埋在 `RequestBuilder` 里的话，唯一的验法是起一个假服务端。
fn create_job_body(
    upload_id: &str,
    lang_hint: Option<&str>,
    tier: luna_core::QueueTier,
) -> serde_json::Value {
    serde_json::json!({
        "upload_id": upload_id,
        "lang_hint": lang_hint,
        // **必填**（ADR-049 ⑥）：服务端没有默认值，漏了它就是 400。
        // 这是刻意的——给它一个默认会让每一个忘了传的调用方买到高优先级，
        // 而「严格按档先行」这条调度语义随之作废。协议版本因此抬到了 2。
        "tier": tier.as_str(),
        // 不报时长：服务端在预留前自己量上传的 WAV（协议 v13），客户端声明的数字会被报短。
    })
}

/// 事件流上会发生的事。
///
/// **做成一条流而不是「跑完再返回一个汇总」**：ADR-049 的排位帧会反复到达，
/// 而它的全部价值就是**即时**——攒到终态再一起交出去，用户在排队那两小时里看到的
/// 仍然只有一个转圈。此前这个函数返回 `(Option<PhaseDto>, Option<ExecutorInfo>)`，
/// 那种形状表达不了「过程中还会有事发生」。
enum WatchEvent {
    /// 谁跑的这次转写（PRD-17-033）。
    Executor(ExecutorInfo),
    /// 服务端队列里的排位（ADR-049 ⑧）。
    Queued(luna_core::RemoteQueueStatus),
    /// 终态帧。收到即结束。
    Terminal(PhaseDto),
    /// 该停了：取消、严格本地、或连接层出错。
    Stop(JobError),
}

/// 订阅阶段事件流，直到终态。
///
/// 流正常结束却没给出终态（服务端重启之类）时**什么都不发**——调用方回落轮询补一次。
/// 连不上/协议不符发一条 [`WatchEvent::Stop`]，同样回落轮询。
/// **SSE 只是省掉空等，不是唯一通路**：反代缓冲 SSE 是常见部署问题，唯一依赖它会让
/// 转写在那些环境里彻底不可用。
fn watch_events<'a>(
    http: &'a reqwest::Client,
    base: &'a str,
    token: &'a str,
    job_id: &'a str,
    interrupt: Interrupt<'a>,
) -> impl futures::Stream<Item = WatchEvent> + 'a {
    stream! {
        use futures::StreamExt;

        let sent = http
            .get(format!("{base}/inference/v1/transcriptions/{job_id}/events"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(net_err);
        let response = match sent {
            Ok(r) => match check_status(r).await {
                Ok(r) => r,
                Err(e) => { yield WatchEvent::Stop(e); return; }
            },
            Err(e) => { yield WatchEvent::Stop(e); return; }
        };

        let mut decoder = crate::sse::SseDecoder::new();
        let mut bytes = response.bytes_stream();
        loop {
            // 取消必须与「等下一帧」**并发**，不能等帧到了才检查。
            //
            // 真机坐实（2026-08-07）：只在帧到达时轮询取消，长任务里两帧之间可能隔十几秒
            // （阶段流本来就没有中间帧），于是取消要等到解码结束才被发现——而那时服务端
            // 已经把整段跑完并**按全额计费**（usage_events 里那条记成 finished / 873 秒）。
            // 用户点了取消却被全额收费，正是取消这条链路存在的理由被反过来打脸。
            //
            // ADR-049 之后这一段更要紧：批量档的任务可以合法地在队列里待上一天，
            // 而那整段时间里一帧都不会有。
            let chunk = tokio::select! {
                biased;
                next = bytes.next() => match next {
                    Some(c) => c,
                    None => break,
                },
                () = tokio::time::sleep(CANCEL_POLL) => {
                    if let Some(stop) = interrupt.reason() {
                        yield WatchEvent::Stop(stop);
                        return;
                    }
                    continue;
                }
            };
            if let Some(stop) = interrupt.reason() {
                yield WatchEvent::Stop(stop);
                return;
            }
            let chunk = match chunk.map_err(net_err) {
                Ok(c) => c,
                Err(e) => { yield WatchEvent::Stop(e); return; }
            };
            let text = String::from_utf8_lossy(&chunk);
            for payload in decoder.push(&text) {
                // **按事件名分派**（ADR-049 ⑧）：这条流上现在有两种载荷形状——
                // `phase`（阶段 + 执行者 + 终态）与 `queued`（排位 + 等待区间）。
                // 此前只有一种，故按形状盲解就够；现在盲解会把排位帧当成一个缺字段的
                // 阶段帧，而 serde 对缺 `state` 的对象只会报错、被下面的 `continue`
                // 静默吃掉——排位于是永远不出现，且没有任何东西报错。
                let crate::sse::SsePayload::Data { event, json } = payload else {
                    continue;
                };
                match event.as_deref() {
                    Some("queued") => {
                        if let Ok(frame) = serde_json::from_str::<QueuedFrameDto>(&json) {
                            yield WatchEvent::Queued(frame.into_status());
                        }
                    }
                    _ => {
                        let Ok(phase) = serde_json::from_str::<PhaseDto>(&json) else {
                            // 认不出的帧跳过而不是杀掉整条流：服务端将来加新帧类型时，
                            // 老客户端应当照常收终态，而不是当场失败。
                            continue;
                        };
                        if let Some(exec) = &phase.executed_by {
                            yield WatchEvent::Executor(ExecutorInfo {
                                display_name: exec.upstream_name.clone(),
                                // 认不出的性质归 Unknown，**不归 LocalRuntime**：把未知
                                // 当成"本地"就是一次实打实的隐私误述
                                // （见 `ExecutorKind::Unknown` 文档）。
                                kind: ExecutorKind::parse(&exec.kind),
                                model: exec.model.clone(),
                                // 恒 `true`：这条回报来自服务端，也就是说音频**已经
                                // 离开这台设备**了。服务端报的 `kind` 说的是它自己那一侧
                                // 的边界（它的 WhisperLive 对它而言是 `local_runtime`），
                                // 拿来当「离开设备了吗」会让指示在音频正在上传时熄灭。
                                leaves_device: true,
                            });
                        }
                        if terminal_state(&phase.state) {
                            yield WatchEvent::Terminal(phase);
                            return;
                        }
                    }
                }
            }
        }
    }
}

/// 服务端终态的**唯一**判据。
///
/// 抽成一处的理由是 ADR-049 加了 `expired`：此前 `finished|failed|cancelled` 这串
/// 字面量在事件流与轮询两处各写了一遍，漏改一处的症状是「过期的任务在那条路上永远
/// 等下去」——SSE 侧挂到超时，轮询侧一直转圈，两种都不报错。
fn terminal_state(state: &str) -> bool {
    matches!(state, "finished" | "failed" | "cancelled" | "expired")
}

async fn poll_job(
    http: &reqwest::Client,
    base: &str,
    bearer: &dyn BearerSource,
    token: &mut String,
    job_id: &str,
) -> Result<StatusDto, JobError> {
    send_authed(bearer, token, |tok| {
        http.get(format!("{base}/inference/v1/transcriptions/{job_id}"))
            .bearer_auth(tok)
    })
    .await?
    .json()
    .await
    .map_err(net_err)
}

async fn cancel_job(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    job_id: &str,
) -> Result<(), JobError> {
    http.delete(format!("{base}/inference/v1/transcriptions/{job_id}"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(net_err)?;
    Ok(())
}

/// 服务端终态错误码 → `JobError`。
///
/// **额度耗尽必须单独认出来**：混进 `Upstream` 的话，客户端会把它当成可重试的
/// 临时故障反复退避重试，而重试永远不会成功（与 `classify_http_failure` 里 402/429
/// 那条是同一个教训）。
fn map_terminal_error(code: Option<&str>) -> JobError {
    match code {
        Some("quota_exhausted") => quota_exhausted(),
        // 本月预付上限（C4）：与 HTTP 402 那条同一判据（见 `payment_required_error`）。
        Some(crate::cloud_llm::SPEND_CAP_REACHED | crate::cloud_llm::FREE_QUOTA_WITHHELD) => {
            payment_required_error(code)
        }
        Some(crate::cloud_llm::CAPABILITY_NOT_ALLOWED | crate::cloud_llm::ACCOUNT_FROZEN) => {
            forbidden_error(code)
        }
        // 与 HTTP 429 同一条：任务在**跑的过程中**撞上限流（上游自己回的 429 被服务端
        // 映射成这个终态码）同样是「等一会儿会好」，不是「上游坏了」。两条路径各判一次
        // 就是两份会漂的映射——服务端的稳定码与 HTTP 状态本来就指向同一件事。
        Some("rate_limited") => JobError::RateLimited {
            provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
            // 终态码里没有 `Retry-After` 可读——不编一个，交给调用方的既有退避策略。
            retry_after_secs: None,
        },
        // 会话在任务跑到一半时失效（租约到期 / 设备被撤销）——同样要落到「去登录」。
        Some("unauthorized") => unauthorized(),
        // 承诺窗口内没排上（ADR-049 ⑦）：**不归可重试的 `Upstream`**——立刻重试
        // 只会把它排到同一条排不动的队伍后面。也不归 `RateLimited`：那条要带一个
        // 「等这么久」的窗口，而服务端恰恰答不上来。
        Some("expired") => JobError::QueueExpired {
            provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
        },
        Some("upstream_unavailable") => JobError::ProviderUnavailable {
            provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
        },
        other => JobError::Upstream {
            provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
            code: other.map(ToOwned::to_owned),
            message: "服务端转写失败".to_owned(),
        },
    }
}

/// 网络层失败 → `JobError`，与对话侧 `llm_http::net_err` 同口径
/// （超时归 `Timeout`、连不上归 `ProviderUnavailable` 以触发兜底路由）。
fn net_err(err: reqwest::Error) -> JobError {
    if err.is_timeout() {
        JobError::Timeout
    } else if err.is_connect() {
        JobError::ProviderUnavailable {
            provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
        }
    } else {
        JobError::Upstream {
            provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
            code: None,
            message: format!("服务端不可达：{}", req_kind(&err)),
        }
    }
}

/// HTTP 状态 → `JobError`。401/403/402/429 与对话侧 `classify_http_failure` 同口径。
///
/// **判定必须在 `error_for_status()` 之前做**，这是本函数收 `Response` 而不是
/// `reqwest::Error` 的全部理由：`Error` 只带得走状态码，而 429 唯一可执行的信息
/// （`Retry-After`）在响应头里——一旦转成 `Error`，那个头就永远取不回来了。
/// 此前这里正是收 `Error` 的，于是 429 只能落进通用 `Upstream`。
async fn check_status(response: reqwest::Response) -> Result<reqwest::Response, JobError> {
    // 403 有两种：凭据被拒（去登录）与组织没开放转写（R4b，回本机）。只有响应体说得清是
    // 哪一种，故 403 时读一次体；其余状态照旧只看状态码与头。
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
        return Err(forbidden_error(
            body.get("error").and_then(serde_json::Value::as_str),
        ));
    }
    // 402 也有两种：额度用尽（升级 / 购买）与撞上用户自设的本月预付上限（调整上限，C4）——
    // 同样只有响应体说得清。
    if response.status() == reqwest::StatusCode::PAYMENT_REQUIRED {
        let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
        return Err(payment_required_error(
            body.get("error").and_then(serde_json::Value::as_str),
        ));
    }
    match classify_status(response.status().as_u16(), response.headers()) {
        Some(err) => Err(err),
        None => Ok(response),
    }
}

/// 403 → 错误：`capability_not_allowed` 是组织策略，`account_frozen` 是账户被冻结（C3，与额度
/// 用尽同一侧不可重试、不回落本机），其余照旧是「凭据没这个权限」。
fn forbidden_error(code: Option<&str>) -> JobError {
    let provider = ProviderId::from(REMOTE_ASR_PROVIDER_ID);
    match code {
        Some(crate::cloud_llm::CAPABILITY_NOT_ALLOWED) => {
            JobError::CapabilityNotAllowed { provider }
        }
        Some(crate::cloud_llm::ACCOUNT_FROZEN) => JobError::AccountFrozen { provider },
        _ => unauthorized(),
    }
}

/// 402 → 错误：`spend_cap_reached` 是用户自设的本月预付上限到了（C4，下一步是调整上限），
/// `free_quota_withheld` 是这个账户此刻领不到免费额度（C5，下一步看成因），其余照旧是额度用尽
/// （下一步是升级或购买）。三者都不可重试、不回落本机。
fn payment_required_error(code: Option<&str>) -> JobError {
    match code {
        Some(crate::cloud_llm::SPEND_CAP_REACHED) => JobError::SpendCapReached {
            provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
        },
        Some(crate::cloud_llm::FREE_QUOTA_WITHHELD) => JobError::FreeQuotaWithheld {
            provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
        },
        _ => quota_exhausted(),
    }
}

/// 状态码 + 响应头 → 错误（`None` = 这不是一次失败）。
///
/// 与 [`check_status`] 分开是为了**测得了**：构造一个 `reqwest::Response` 要拉进 `http`
/// crate，而这里真正要钉的判据只用得上状态码与头两样东西。
fn classify_status(status: u16, headers: &reqwest::header::HeaderMap) -> Option<JobError> {
    if !(400..600).contains(&status) {
        return None;
    }
    Some(match status {
        // 401/403 归 `Unauthorized`（TDD-16 §5.3 映射表）：UI 据此给「去登录」这个可执行
        // 动作，而不是一句「上游出错了」；同时它不可重试——凭据没换，重试多少次都一样。
        // 403 是「认了这份凭据，但它没这个权限」（档位不含服务端转写、设备已被撤销）。
        401 | 403 => unauthorized(),
        // 402 = 额度用尽，归 `QuotaExhausted` 而不是 `Upstream`：后者可重试，而额度
        // 不会因为再试一次而回来。也不是 `ResourceExhausted`——那条的建议（换小模型 /
        // 释放显存）对额度用尽的用户毫无意义。
        402 => quota_exhausted(),
        // 429 = 服务端的速率闸或**上游并发闸门**（满则换候选、全满即此），与 402 相反：
        // 它**会**因为等一会儿再来而恢复。归进 `Upstream` 的代价是双向的——前端读的是变体
        // 投影出的稳定码，`Upstream.code` 里那个 "429" 它根本看不到，于是「稍后自动重试」
        // 退化成一句「上游出错」；而服务端明明在 `Retry-After` 里给出了等多久。
        429 => JobError::RateLimited {
            provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
            // 与对话/嵌入侧共用同一份解析（整数秒与 HTTP-date 两种合法形状都认），
            // 不在这里另写一遍。
            retry_after_secs: crate::llm_http::retry_after_secs(headers),
        },
        code => JobError::Upstream {
            provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
            code: Some(code.to_string()),
            message: "服务端返回错误".to_owned(),
        },
    })
}

/// 「额度用尽」的统一构造。转写这条通道超的一定是 `asr_seconds` 维——服务端 402 的
/// `dimension` 与终态码两条路都指向同一句话，故维度名只写一处。
fn quota_exhausted() -> JobError {
    JobError::QuotaExhausted {
        dimension: Some(ASR_QUOTA_DIMENSION.to_owned()),
    }
}

/// 转写消耗的配额维度名（服务端 `usage_events` 的同名维，TDD-16 §7.2）。
const ASR_QUOTA_DIMENSION: &str = "asr_seconds";

fn req_kind(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        "timeout"
    } else if err.is_connect() {
        "connect"
    } else if err.is_decode() {
        "decode"
    } else {
        "other"
    }
}

fn once_failed(error: JobError) -> JobEventStream {
    Box::pin(stream! { yield JobEvent::Failed(error); })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 403 的两种含义只有响应体分得清（R4b）：组织没开放转写 → 回本机；其余 → 去登录。
    #[test]
    fn a_forbidden_body_tells_policy_from_credentials() {
        assert!(matches!(
            forbidden_error(Some("capability_not_allowed")),
            JobError::CapabilityNotAllowed { .. }
        ));
        assert!(matches!(
            forbidden_error(Some("forbidden")),
            JobError::Unauthorized { .. }
        ));
        assert!(matches!(
            forbidden_error(None),
            JobError::Unauthorized { .. }
        ));
        assert!(matches!(
            map_terminal_error(Some("capability_not_allowed")),
            JobError::CapabilityNotAllowed { .. }
        ));
        // 本月预付上限（C4）：HTTP 402 体与任务终态码两条路都认；402 别的体照旧是额度用尽。
        for capped in [
            payment_required_error(Some("spend_cap_reached")),
            map_terminal_error(Some("spend_cap_reached")),
        ] {
            assert!(
                matches!(capped, JobError::SpendCapReached { .. }),
                "{capped:?}"
            );
            assert!(!capped.is_retriable());
        }
        // 免费额度被扣下（C5）：同样两条路都认，且不落成「额度用尽」。
        for withheld in [
            payment_required_error(Some("free_quota_withheld")),
            map_terminal_error(Some("free_quota_withheld")),
        ] {
            assert!(
                matches!(withheld, JobError::FreeQuotaWithheld { .. }),
                "{withheld:?}"
            );
            assert!(!withheld.is_retriable() && withheld.is_structural());
        }
        for quota in [
            payment_required_error(Some("quota_exhausted")),
            payment_required_error(None),
        ] {
            assert!(
                matches!(quota, JobError::QuotaExhausted { .. }),
                "{quota:?}"
            );
        }
        // 账户被冻结（C3）：HTTP 403 体与任务终态码两条路都认，且不落成「去登录」。
        for frozen in [
            forbidden_error(Some("account_frozen")),
            map_terminal_error(Some("account_frozen")),
        ] {
            assert!(
                matches!(frozen, JobError::AccountFrozen { .. }),
                "{frozen:?}"
            );
            assert!(!frozen.is_retriable());
        }
    }

    /// `location` 必须是 `Cloud`。
    ///
    /// 严格本地模式按它过滤（既有机制）。标成 `Local` 时编译、lint、其余单测全绿，
    /// 而"仅本地"下用户的录音照样被发出去——本模块最严重的一种失败，且无任何报错。
    #[test]
    fn descriptor_is_honestly_cloud() {
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
        let p = RemoteAsrProvider::new(RemoteAsrConfig {
            api_base: "https://example.test".into(),
            bearer: Arc::new(NoToken),
            gate: Arc::new(luna_core::AllowOutbound),
        });
        let d = p.descriptor();
        assert_eq!(d.location, ProviderLocation::Cloud);
        let caps = p.capabilities();
        assert_eq!(
            caps.location,
            ProviderLocation::Cloud,
            "能力集里的 location 同样要诚实"
        );
        assert!(caps.capabilities.contains(&Capability::Transcribe));
        assert!(
            !caps.streaming,
            "批式通道不产生增量——声明 streaming 会让 UI 等一个永不到来的 partial"
        );
    }

    /// WAV 头逐字段钉死。
    ///
    /// 写错一个字节，服务端交给 whisper 只会回一句语焉不详的格式错，而根因在客户端
    /// 拼头的那 44 字节里——那是最难从错误信息倒推回来的一类。
    #[test]
    fn wav_header_is_byte_exact() {
        let pcm = luna_core::PcmBuffer {
            sample_rate: 16000,
            channels: 1,
            samples: vec![0.0, 0.5, -0.5],
        };
        let wav = pcm_to_wav(&pcm);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + 3 * 2, "头 44 字节 + 每样本 2 字节");
        // RIFF 长度 = 文件长 - 8；data 长度 = 样本字节数。两者错位是最常见的拼头 bug。
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 6);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
        assert_eq!(
            u16::from_le_bytes(wav[20..22].try_into().unwrap()),
            1,
            "PCM 格式"
        );
        assert_eq!(
            u16::from_le_bytes(wav[22..24].try_into().unwrap()),
            1,
            "声道"
        );
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16000);
        assert_eq!(
            u32::from_le_bytes(wav[28..32].try_into().unwrap()),
            32000,
            "字节率"
        );
        assert_eq!(
            u16::from_le_bytes(wav[32..34].try_into().unwrap()),
            2,
            "块对齐"
        );
        assert_eq!(
            u16::from_le_bytes(wav[34..36].try_into().unwrap()),
            16,
            "位深"
        );
        // 样本值：0.5 → 16383（i16::MAX 的一半）
        assert_eq!(i16::from_le_bytes(wav[46..48].try_into().unwrap()), 16383);
        assert_eq!(i16::from_le_bytes(wav[48..50].try_into().unwrap()), -16383);
    }

    /// 超幅样本必须裁剪，不能让它回绕。
    ///
    /// 不裁剪时 `1.5 * i16::MAX` 溢出后回绕成一个大负数——听感上是反相噪声，
    /// 表现为「转写突然听不懂了」，而音频文件本身看起来完全正常。
    #[test]
    fn out_of_range_samples_clamp_instead_of_wrapping() {
        let pcm = luna_core::PcmBuffer {
            sample_rate: 16000,
            channels: 1,
            samples: vec![1.5, -1.5],
        };
        let wav = pcm_to_wav(&pcm);
        assert_eq!(
            i16::from_le_bytes(wav[44..46].try_into().unwrap()),
            i16::MAX
        );
        assert_eq!(
            i16::from_le_bytes(wav[46..48].try_into().unwrap()),
            -i16::MAX,
            "超幅的负样本同样要裁到边界，而不是回绕"
        );
    }

    /// PCM 现在是**被接受**的来源——批量流水线只会给这个。
    #[test]
    fn pcm_is_accepted_because_the_pipeline_only_gives_pcm() {
        let src = audio_source(&AudioRef::Pcm(luna_core::PcmBuffer {
            sample_rate: 16000,
            channels: 1,
            samples: vec![0.0],
        }))
        .expect("批量转写流水线一律提交 Pcm，拒绝它等于做了个没人走的 Provider");
        assert!(matches!(src, Source::Pcm(_)));
    }

    /// 实时流被拒，且说明指向正确的通道（不是一句"输入无效"）。
    #[test]
    fn rejects_stream_with_actionable_reason() {
        let stream = audio_source(&AudioRef::Stream(luna_core::StreamId::new()));
        let JobError::InvalidInput(msg) = stream.unwrap_err() else {
            panic!("应是 InvalidInput");
        };
        assert!(msg.contains("实时"), "要指向实时通道，实得：{msg}");
    }

    /// `Uploaded` 直接建任务——不重传是这个变体存在的全部理由。
    #[test]
    fn uploaded_variant_skips_upload() {
        let src = audio_source(&AudioRef::Uploaded(luna_core::UploadId::from("up_1"))).unwrap();
        assert!(matches!(src, Source::Uploaded(id) if id == "up_1"));
    }

    /// 建任务的请求体不报时长：时长由服务端量，客户端的数字没有人读（协议 v13 硬切）。
    #[test]
    fn create_body_does_not_declare_a_duration() {
        let body = create_job_body("up_1", Some("zh"), luna_core::QueueTier::Batch);
        assert_eq!(body["upload_id"], "up_1");
        assert_eq!(body["tier"], "batch");
        assert!(body.get("declared_seconds").is_none(), "{body}");
    }

    /// 额度耗尽映射成 `ResourceExhausted` 而不是 `Upstream`。
    ///
    /// 归到 `Upstream` 的话客户端会当成临时故障反复退避重试，而重试永远不会成功
    /// ——与 `classify_http_failure` 里 402/429 那条是同一个教训。
    #[test]
    fn quota_exhausted_is_actionable_not_retryable() {
        assert!(matches!(
            map_terminal_error(Some("quota_exhausted")),
            JobError::QuotaExhausted { dimension: Some(d) } if d == "asr_seconds"
        ));
        // 未登录/凭据失效 → `Unauthorized`（UI 据此给「去登录」，而不是一句「上游出错了」
        // 让用户无从下手）。它同样不可重试：凭据没换，重试还是 401。
        let unauthorized = map_terminal_error(Some("unauthorized"));
        assert!(matches!(unauthorized, JobError::Unauthorized { .. }));
        assert_eq!(unauthorized.code(), "SIGN_IN_REQUIRED");
        assert!(!unauthorized.is_retriable());
        // 未知码保留原文进 `code`，不吞掉——运维要能据此定位。
        assert!(matches!(
            map_terminal_error(Some("weird_new_code")),
            JobError::Upstream { code: Some(c), .. } if c == "weird_new_code"
        ));
    }

    /// **刷新令牌换不出会话时，收场必须与「未登录」完全同一档。**
    ///
    /// 这条钉的是 [`unauthorized`]。它此前返回的是带 `code: Some("UNAUTHORIZED")` 的
    /// `Upstream`，而稳定码取自**变体**——塞在 `Upstream.code` 里的那个字符串前端根本
    /// 读不到，「去登录」的深链于是退化成一句「上游出错」；更贵的是 `Upstream` 可重试，
    /// JobQueue 会反复重发一个凭据没换就必然再 401 的请求。
    ///
    /// 断言写在 `code()` / `is_retriable()` 这两个**投影**上而不是变体名上：会退化的
    /// 正是这两个性质，而变体名换了也不一定错。
    #[test]
    fn a_failed_refresh_lands_in_the_same_bucket_as_never_logged_in() {
        let refreshed = unauthorized();
        let never = JobError::Unauthorized {
            provider: ProviderId::from(REMOTE_ASR_PROVIDER_ID),
        };
        assert_eq!(
            refreshed.code(),
            never.code(),
            "刷新失败与未登录给出了不同的稳定码——UI 只会对其中一条给出「去登录」"
        );
        assert!(
            !refreshed.is_retriable(),
            "刷新失败被判为可重试——凭据没换，重试永远是 401，用户看到的是无声的卡死"
        );
    }

    /// 等待上限必须与轮询间隔**算得出来**，不能各写各的。
    ///
    /// 手写的那版是 `4 * 60 * 40 // ≈ 1 小时`，实际 4 小时——注释与常量各说各话，
    /// 而没有任何东西会发现。这条把「两个数一致」变成可断言的。
    #[test]
    fn poll_budget_matches_the_declared_wait() {
        let actual = Duration::from_millis(u64::from(MAX_POLLS) * POLL_INTERVAL.as_millis() as u64);
        assert_eq!(
            actual, MAX_WAIT,
            "轮询预算 {actual:?} 与声明的等待上限 {MAX_WAIT:?} 不一致"
        );
        // MAX_POLLS 是编译期常量，直接 `> 0` 会被 clippy 判为恒真断言；
        // 真正要防的是「间隔被调大到超过上限」导致一次都不轮询。
        assert!(
            POLL_INTERVAL <= MAX_WAIT,
            "轮询间隔 {POLL_INTERVAL:?} 超过等待上限 {MAX_WAIT:?} → 一次都不会轮询"
        );
    }

    /// 容忍窗口要够长到跨过一次常见抖动，又不能长到把真断线拖成长时间假等待。
    #[test]
    fn transient_failure_window_is_a_sane_duration() {
        let window = Duration::from_millis(
            u64::from(MAX_CONSECUTIVE_POLL_FAILURES) * POLL_INTERVAL.as_millis() as u64,
        );
        assert!(
            window >= Duration::from_secs(10) && window <= Duration::from_secs(60),
            "连续失败容忍窗口 {window:?} 不合理——太短会被一次抖动杀掉（而服务端还在跑\
             还在计费），太长会让真断线拖成长时间假等待"
        );
    }

    /// 上游不可用 → `ProviderUnavailable`，这样 Router 的兜底路由才会接管
    /// （回落本机 whisper）。归 `Upstream` 的话兜底不触发，用户直接看到失败。
    #[test]
    fn upstream_unavailable_triggers_local_fallback() {
        assert!(matches!(
            map_terminal_error(Some("upstream_unavailable")),
            JobError::ProviderUnavailable { .. }
        ));
    }

    /// 429 必须归 `RateLimited` **并把 `Retry-After` 带出来**。
    ///
    /// 服务端在两处会发它：账户级速率闸，以及上游并发闸门（满则换候选、全满即此）。
    /// 归进 `Upstream` 的代价是双向的——前端读的是变体投影出的稳定码，`Upstream.code`
    /// 里那个 "429" 它根本看不到，于是「稍后自动重试」退化成一句「上游出错」；
    /// 而服务端明明已经在 `Retry-After` 里说了等多久。
    ///
    /// 这条同时钉住判定发生在 `error_for_status()` **之前**：`reqwest::Error` 带不走响应头，
    /// 一旦先转成 Error，下面这个 `Some(3)` 就永远只能是 `None`。
    #[test]
    fn rate_limited_carries_the_servers_retry_after() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "3".parse().unwrap());
        let err = classify_status(429, &headers).expect("429 是失败");
        assert!(
            matches!(
                err,
                JobError::RateLimited {
                    retry_after_secs: Some(3),
                    ..
                }
            ),
            "429 必须带着服务端给的等待秒数落到 RateLimited：{err:?}"
        );
    }

    /// 没有 `Retry-After` 时**不编一个**——交给调用方的既有退避策略。
    #[test]
    fn rate_limited_without_a_header_does_not_invent_a_delay() {
        assert!(matches!(
            classify_status(429, &reqwest::header::HeaderMap::new()),
            Some(JobError::RateLimited {
                retry_after_secs: None,
                ..
            })
        ));
    }

    /// 429 与 402 必须分开：前者等一会儿会好，后者重试一万次也不会。
    ///
    /// 混成一条的后果是对称的两种错法——把额度用尽当限流会让客户端永远退避重试，
    /// 把限流当额度用尽会让它放弃并提示用户去加额度。
    #[test]
    fn payment_required_is_not_a_rate_limit() {
        assert!(matches!(
            classify_status(402, &reqwest::header::HeaderMap::new()),
            Some(JobError::QuotaExhausted { dimension: Some(d) }) if d == ASR_QUOTA_DIMENSION
        ));
    }

    /// 401/403 归「去登录」，且**不可重试**——会话没换，重试多少次都一样。
    #[test]
    fn credential_failures_route_to_sign_in() {
        for status in [401, 403] {
            let err = classify_status(status, &reqwest::header::HeaderMap::new());
            assert!(
                matches!(err, Some(JobError::Unauthorized { .. })),
                "{status} 必须落到 Unauthorized"
            );
            assert_eq!(err.map(|e| e.code()), Some("SIGN_IN_REQUIRED"), "{status}");
        }
    }

    /// 跑到一半撞上限流（服务端终态码 `rate_limited`）与 HTTP 429 是同一件事，
    /// 必须映射到同一个变体——两条路径各判一次就是两份会漂的映射。
    #[test]
    fn terminal_rate_limited_matches_the_http_one() {
        assert!(matches!(
            map_terminal_error(Some("rate_limited")),
            JobError::RateLimited { .. }
        ));
    }

    /// 2xx 不是失败——`classify_status` 只在失败时构造错误。
    #[test]
    fn success_is_not_an_error() {
        assert!(classify_status(200, &reqwest::header::HeaderMap::new()).is_none());
    }

    /// **服务端没给等待区间时，客户端不得凭空造一个**。
    ///
    /// 三处（建任务响应 / 轮询响应 / SSE 帧）共用同一份换算正是为了这一条：其中一处
    /// 忘了把缺席保持成 `None`，界面上就会出现「预计 0–0 分钟」——比不显示更糟。
    #[test]
    fn a_queue_frame_without_a_range_stays_without_one() {
        let frame: QueuedFrameDto =
            serde_json::from_str(r#"{"state":"queued","position":4,"tier":"batch"}"#).unwrap();
        let status = frame.into_status();
        assert_eq!(status.position, 4);
        assert_eq!(status.tier, luna_core::QueueTier::Batch);
        assert!(status.eta.is_none(), "服务端没给区间，客户端不得编一个");

        let with_eta: QueuedFrameDto = serde_json::from_str(
            r#"{"state":"queued","position":1,"tier":"standard",
                "eta_range":{"min_secs":60,"max_secs":180}}"#,
        )
        .unwrap();
        let status = with_eta.into_status();
        assert_eq!(
            status.eta,
            Some(luna_core::EtaRange {
                min_secs: 60,
                max_secs: 180
            })
        );
    }

    /// 建任务响应里的排位与 SSE 帧走**同一份换算**（形状不同、语义相同）。
    #[test]
    fn the_create_response_carries_the_first_position() {
        let dto: CreateJobDto = serde_json::from_str(
            r#"{"job_id":"j-1","state":"queued",
                "queue":{"position":2,"tier":"batch","eta_range":{"min_secs":10,"max_secs":20}}}"#,
        )
        .unwrap();
        let queue = dto.queue.expect("建任务响应带排位").into_status();
        assert_eq!(queue.position, 2);
        assert_eq!(queue.tier, luna_core::QueueTier::Batch);

        // 服务端没排队（交互车道或直接开跑）时字段整个缺席，不是一个位次为 0 的对象。
        let direct: CreateJobDto = serde_json::from_str(r#"{"job_id":"j-2"}"#).unwrap();
        assert!(direct.queue.is_none());
    }

    /// **终态判据只有一份**（`terminal_state`）。
    ///
    /// ADR-049 加了 `expired`，而此前 `finished|failed|cancelled` 这串字面量在事件流与
    /// 轮询两处各写了一遍。漏改一处的症状是「过期的任务在那条路上永远等下去」——
    /// SSE 侧挂到超时，轮询侧一直转圈，两种都不报错。
    #[test]
    fn expiry_is_a_terminal_state_on_both_paths() {
        for state in ["finished", "failed", "cancelled", "expired"] {
            assert!(terminal_state(state), "`{state}` 必须是终态");
        }
        for state in ["queued", "running", ""] {
            assert!(!terminal_state(state), "`{state}` 不是终态");
        }
    }

    // ── 取结果：`segments` 缺席 ≠ 空 ────────────────────────────────────────────
    //
    // 这三条起**真的 TCP 假服务端**，而不是把 `poll_job` 抽出来单独验一个 DTO。
    // 缺陷恰恰不在解析里：`StatusDto` 一直老老实实把 `null` 解成 `None`，是**拿到
    // `None` 之后**的那一句 `unwrap_or_default()` 把它抹平成了空结果。把中间任何一段
    // 换成假实现，都正好绕开出事的那个接缝。走 `AudioRef::Uploaded` 是为了跳过上传
    // （那是这个变体存在的理由），让往返只剩「建任务 → 事件流 → 取结果」三步。

    /// 一个只够本文件用的假 Lunarca 服务端：按**请求行**路由，响应体由调用方给。
    ///
    /// 同一条连接上可能连着来好几个请求（reqwest 复用连接），故逐个请求读、逐个应答。
    /// 读到的分片若不是以 HTTP 方法开头（POST 的体可能单独成片到达），跳过不应答——
    /// 否则一个请求会被回两次响应，症状是后面所有往返整体错位一格。
    async fn fake_server<F>(route: F) -> String
    where
        F: Fn(&str) -> String + Send + Sync + 'static,
    {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let route = Arc::new(route);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let route = Arc::clone(&route);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 64 * 1024];
                    loop {
                        let Ok(read) = socket.read(&mut buf).await else {
                            return;
                        };
                        if read == 0 {
                            return;
                        }
                        let request = String::from_utf8_lossy(&buf[..read]).into_owned();
                        let line = request.lines().next().unwrap_or_default();
                        if !["GET ", "POST ", "DELETE "]
                            .iter()
                            .any(|m| line.starts_with(m))
                        {
                            continue;
                        }
                        if socket.write_all(route(line).as_bytes()).await.is_err() {
                            return;
                        }
                        let _ = socket.flush().await;
                    }
                });
            }
        });
        format!("http://{address}")
    }

    fn json_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    /// 一帧就结束的事件流：`Content-Length` 发完即体尽，`bytes_stream()` 自然收口。
    fn sse_response(frame: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{frame}",
            frame.len()
        )
    }

    fn one_segment_json() -> String {
        // 用**真的** `Segment` 序列化，而不是手抄一份 JSON：手抄的那份一旦与线上形状
        // 分叉，本用例会绿着骗人（`Duration` 的 `{secs,nanos}` 正是这样出过事的）。
        serde_json::to_string(&vec![Segment {
            id: luna_core::SegmentId::new(),
            start: Duration::from_secs(0),
            end: Duration::from_secs(2),
            text: "有人说话了".to_owned(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        }])
        .unwrap()
    }

    struct StaticToken;
    #[async_trait]
    impl BearerSource for StaticToken {
        async fn bearer(&self) -> Option<String> {
            Some("tok".to_owned())
        }
        async fn refresh(&self, _rejected: &str) -> Option<String> {
            Some("tok".to_owned())
        }
    }

    /// 跑一次「已上传的音频 → 取结果」，收齐全部事件。
    async fn run_uploaded(api_base: String) -> Vec<JobEvent> {
        use futures::StreamExt as _;
        let provider = RemoteAsrProvider::new(RemoteAsrConfig {
            api_base,
            bearer: Arc::new(StaticToken),
            gate: Arc::new(luna_core::AllowOutbound),
        });
        let job = Job::new(
            JobKind::Transcribe {
                audio: AudioRef::Uploaded(luna_core::UploadId::from("up_1")),
                lang_hint: None,
                initial_prompt: None,
            },
            luna_core::JobOrigin::SecondPass,
        );
        let mut stream = provider.execute(job, CancelToken::new()).await;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        events
    }

    /// `finished` + `segments: null` = **结果取不回来**，绝不是「转写出来是空的」。
    ///
    /// 现网 3/3 复现的那件事就在这里：24 分钟的录音、服务端 `finished`、1448 秒、
    /// 上游 AssemblyAI，而客户端把 `None` `unwrap_or_default()` 成 `[]`，一路
    /// `Finished(Segments([]))` 写进笔记——vault 里留下一个空的转写块和一个写满了的
    /// `model` 字段，用户看到「暂无转写内容」而**没有任何错误**。
    ///
    /// 反向验证（实测）：把 `let Some(segments) = … else` 换回 `unwrap_or_default()`，
    /// 本用例当场红在「不该是 Finished」那一句上。
    #[tokio::test]
    async fn a_finished_job_without_segments_is_a_failure_not_an_empty_transcript() {
        let base = fake_server(|line| {
            if line.starts_with("POST ") {
                json_response(r#"{"job_id":"job-1"}"#)
            } else if line.contains("/events") {
                sse_response("data: {\"state\":\"finished\"}\n\n")
            } else {
                // 服务端如实说「结果不在」——缺席，而不是一个空数组。
                json_response(r#"{"state":"finished","segments":null}"#)
            }
        })
        .await;

        let events = run_uploaded(base).await;
        assert!(
            !events.iter().any(|e| matches!(e, JobEvent::Finished(_))),
            "结果取不回来却报了 Finished——这一份会被当成权威转写写进 Vault：{events:?}"
        );
        let failure = events
            .iter()
            .find_map(|e| match e {
                JobEvent::Failed(err) => Some(err),
                _ => None,
            })
            .expect("必须以一个明确的失败收场，而不是静默的空结果");
        assert!(
            matches!(failure, JobError::ResultUnavailable { .. }),
            "实得 {failure:?}"
        );
        assert_eq!(failure.code(), "RESULT_UNAVAILABLE");
        assert!(
            !failure.is_retriable(),
            "自动重试拿不回已经不存在的结果，却会再计一次费"
        );
    }

    /// `finished` + `segments: []` 仍是**合法的成功**：真的没人说话（服务端对纯静音
    /// 另有兜底）。把它一并判成失败，就是用一个反向的谎替掉原来那个谎。
    #[tokio::test]
    async fn an_empty_segment_list_is_still_a_successful_transcription() {
        let base = fake_server(|line| {
            if line.starts_with("POST ") {
                json_response(r#"{"job_id":"job-1"}"#)
            } else if line.contains("/events") {
                sse_response("data: {\"state\":\"finished\"}\n\n")
            } else {
                json_response(r#"{"state":"finished","segments":[]}"#)
            }
        })
        .await;

        let events = run_uploaded(base).await;
        assert!(
            !events.iter().any(|e| matches!(e, JobEvent::Failed(_))),
            "空结果是合法的，不该报错：{events:?}"
        );
        let finished = events
            .iter()
            .find_map(|e| match e {
                JobEvent::Finished(result) => Some(result),
                _ => None,
            })
            .expect("应当收到 Finished");
        assert!(
            matches!(&finished.payload, ResultPayload::Segments(segs) if segs.is_empty()),
            "实得 {:?}",
            finished.payload
        );
    }

    /// SSE 说 `finished`、GET 却回 `running` = **服务端状态尚未就绪**，回落轮询。
    ///
    /// 判据：SSE 帧只是一次广播，GET 那一侧才是「任务行 + 结果缓存」的原子读。服务端
    /// 此前会在落行之前就把终态帧发出来，于是客户端紧跟着的那一次 GET 拿到的是
    /// `state:"running", segments:null`。把 SSE 当成比 GET 更权威的事实，就会把这个
    /// **时序窗口**判成终局：判空就是现网那个空转写块，判失败则是把一份跑完了的、
    /// 已经计过费的转写扔掉。轮询本来就存在、本来就是原子的，等它一拍即可。
    ///
    /// 反向验证（实测）：删掉 `Ok(status) if status.state != "finished" => {}` 这一臂
    /// （即回到「GET 一回来就下结论」），本用例立刻红——拿到的是 `ResultUnavailable`
    /// 而不是分段。
    #[tokio::test]
    async fn an_unsettled_server_state_falls_back_to_polling_instead_of_concluding() {
        let plain_gets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probe = Arc::clone(&plain_gets);
        let base = fake_server(move |line| {
            if line.starts_with("POST ") {
                return json_response(r#"{"job_id":"job-1"}"#);
            }
            if line.contains("/events") {
                return sse_response("data: {\"state\":\"finished\"}\n\n");
            }
            let nth = probe.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if nth == 0 {
                // 终态帧已经广播，但任务行还没落——GET 如实说「还在跑」。
                json_response(r#"{"state":"running","segments":null}"#)
            } else {
                json_response(&format!(
                    r#"{{"state":"finished","segments":{}}}"#,
                    one_segment_json()
                ))
            }
        })
        .await;

        let events = run_uploaded(base).await;
        assert!(
            !events.iter().any(|e| matches!(e, JobEvent::Failed(_))),
            "状态未就绪不是失败——这份转写服务端已经跑完并计了费：{events:?}"
        );
        let finished = events
            .iter()
            .find_map(|e| match e {
                JobEvent::Finished(result) => Some(result),
                _ => None,
            })
            .expect("轮询应当把这份结果等出来");
        let ResultPayload::Segments(segments) = &finished.payload else {
            panic!("应回 Segments，实得 {:?}", finished.payload);
        };
        assert_eq!(segments.len(), 1, "轮询拿到的应是真实分段");
        assert_eq!(segments[0].text, "有人说话了");
        assert!(
            plain_gets.load(std::sync::atomic::Ordering::SeqCst) >= 2,
            "只 GET 了一次说明客户端把 SSE 帧当成了终局，根本没回落轮询"
        );
    }

    /// 过期归 `QueueExpired`，**不可自动重试**。
    ///
    /// 归可重试的 `Upstream` 会让队列白白烧掉候选链，而立刻重试只是把它排到同一条
    /// 排不动的队伍后面；归 `RateLimited` 则要求一个服务端根本给不出的等待窗口。
    #[test]
    fn expired_is_not_something_to_retry_automatically() {
        let err = map_terminal_error(Some("expired"));
        assert!(matches!(err, JobError::QueueExpired { .. }), "{err:?}");
        assert_eq!(err.code(), "QUEUE_EXPIRED");
        assert!(
            !err.is_retriable(),
            "自动重试只会把它排到同一条排不动的队伍后面"
        );
    }
}
