//! 服务端嵌入 Provider（PRD-17-030，纵切 2 收口 ⑥）。
//!
//! 打 Lunarca 服务端的 `POST /inference/v1/embeddings`。与 [`crate::cloud_llm`] 同一条
//! 原生推理面、同一套凭据与外发闸；与 [`crate::embed_http`]（本机随包引擎）是同一个
//! `Capability::Embed` 的两种落点，由装配层三选一，**同一时刻只注册一个**。
//!
//! # 为什么单独一个 Provider 而不是给 `CloudLlmProvider` 加个分支
//!
//! 与本机侧完全同构的理由：嵌入既无 messages 也无流（一次请求一批向量），塞进那条
//! 「合成 messages → 流式 token」的路径等于给它加一条谁都不走的分支。服务端也正是
//! 按同一条判据把嵌入拆成了独立端点。
//!
//! # 它存在的意义：旧设备上的语义索引
//!
//! 全库重建索引在一台 8 GB 的老机器上是分钟级的等待，而这正是用户买算力的原因。
//! 嵌入此前**只能本机**——「算力交给服务端」于是对语义检索完全不生效，用户在设置里
//! 看到「AI 由服务端提供」，重建索引却仍在他自己的 CPU 上跑。

use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use luna_core::{
    AIProvider, CancelToken, Capability, CapabilitySet, Job, JobError, JobEvent, JobEventStream,
    JobKind, JobResult, ProviderDescriptor, ProviderHealth, ProviderId, ProviderLocation,
    ResultPayload, Timestamp, Timings, UsageEstimate,
};
use luna_model_fetch::pinned_tls::PinnedClient;
use serde_json::Value;

use crate::asr_remote::BearerSource;
use crate::cloud_llm::{executor_from, map_server_error_with_retry, CallSite, NATIVE_PATH};

/// Provider id。与本机 `local-embed` 分开——两者落点不同，日志与用量归因要分得清。
pub const CLOUD_EMBED_PROVIDER_ID: &str = "lunarca-cloud-embed";

/// 连接建立超时（与其余网络 Provider 同口径）。
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// 单批请求的整体超时。嵌入不是流，故用总时长。
///
/// **必须严格大于服务端 → 上游那一层的上限**（`ip-gateway` 的 `EMBED_TIMEOUT_SECS`
/// = 120 秒）。超时要由内向外递增：内层先放弃，外层才有机会把它翻译成一个有原因的
/// 错误。两边同值（此前正是如此）会撞出最坏的一种交错——上游第 119 秒回来、服务端正在
/// 结算与序列化，客户端第 120 秒放弃：用户看到超时并重试，而**这一次服务端照常计费**，
/// 重试再算一遍，双倍。留出的余量覆盖服务端侧的结算、序列化与网络回程。
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(150);

pub struct CloudEmbedConfig {
    /// Lunarca 服务端基址（控制面地址，不含 `/inference/v1`）。
    pub api_base: String,
    pub display_name: String,
    /// 稳定向量 Profile；它约束向量空间，不指定服务端具体执行模型。
    pub profile_id: String,
    pub bearer: Arc<dyn BearerSource>,
    pub gate: Arc<dyn luna_core::OutboundGate>,
}

pub struct CloudEmbedProvider {
    config: CloudEmbedConfig,
    /// 自签部署钉住的根证书在这里生效（ADR-153）。
    http: PinnedClient,
    breaker: Arc<crate::breaker::Breaker>,
}

impl CloudEmbedProvider {
    #[must_use]
    pub fn new(config: CloudEmbedConfig) -> Self {
        Self {
            config,
            breaker: Arc::new(crate::breaker::Breaker::new()),
            http: PinnedClient::new(|builder| {
                builder
                    .connect_timeout(CONNECT_TIMEOUT)
                    .timeout(REQUEST_TIMEOUT)
            }),
        }
    }
}

/// 从服务端响应取出向量批。
///
/// **条数对不上即失败**，与本机侧同一条纪律：调用方拿它更新「第 n 片 → 向量」的索引，
/// 静默少一条会让其后每一片都错位，而症状只是"这篇搜不太准"——不报错、也几乎无法归因。
/// 维度不齐同理（半个索引用 1024 维、半个用 768 维，余弦从此没有意义）。
#[must_use]
pub fn parse_cloud_embeddings(json: &Value, expected: usize) -> Option<Vec<Vec<f32>>> {
    let rows = json.get("vectors")?.as_array()?;
    if rows.len() != expected {
        return None;
    }
    let mut out: Vec<Vec<f32>> = Vec::with_capacity(rows.len());
    for row in rows {
        let vector: Vec<f32> = row
            .as_array()?
            .iter()
            .map(|n| n.as_f64().map(|f| f as f32))
            .collect::<Option<Vec<f32>>>()?;
        if vector.is_empty() || out.first().is_some_and(|first| first.len() != vector.len()) {
            return None;
        }
        out.push(vector);
    }
    Some(out)
}

#[async_trait]
impl AIProvider for CloudEmbedProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(CLOUD_EMBED_PROVIDER_ID)
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id(),
            display_name: self.config.display_name.clone(),
            // 原生云端模型由服务端选择，客户端描述符不得冒充已知具体模型。
            model: None,
            // 标错这一项，严格本地模式就拦不住它。
            location: ProviderLocation::Cloud,
            license_summary: None,
        }
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet {
            capabilities: [Capability::Embed].into_iter().collect(),
            // 非流式：一次请求一批向量。
            streaming: false,
            location: ProviderLocation::Cloud,
            explicit_only: false,
        }
    }

    async fn health(&self) -> ProviderHealth {
        // 不主动探活（ADR-037）：可用性由真实请求暴露，失败计数熔断表达。
        self.breaker.health()
    }

    async fn execute(&self, job: Job, cancel: CancelToken) -> JobEventStream {
        let provider_id = self.id();
        let texts = match &job.kind {
            JobKind::Embed { texts, .. } => texts.clone(),
            _ => Vec::new(),
        };

        // 严格本地：一个请求都不发，且在取凭据之前判。
        //
        // 嵌入的输入是**笔记正文的分片**——隐私量级与对话正文相同，不因为它"只是用来
        // 建索引"就低一档。
        if !self.config.gate.allows_outbound() {
            return once_failed(JobError::StrictLocalBlocked {
                capability: Capability::Embed,
            });
        }
        if texts.is_empty() {
            return once_failed(JobError::InvalidInput(
                "嵌入 Provider 只接受 JobKind::Embed 且文本批不得为空".to_owned(),
            ));
        }
        let Some(token) = self.config.bearer.bearer().await else {
            return once_failed(JobError::Unauthorized {
                provider: provider_id,
            });
        };

        let http = self.http.for_url(&self.config.api_base);
        let breaker = Arc::clone(&self.breaker);
        let bearer = Arc::clone(&self.config.bearer);
        let gate = Arc::clone(&self.config.gate);
        let base = self.config.api_base.trim_end_matches('/').to_owned();
        let profile_id = self.config.profile_id.clone();
        let job_id = job.id;
        // 嵌入端点没有 `service_class` 这个 wire 字段（它不是 `/jobs`），但错误映射那侧
        // 要能说出「空的是哪一档的哪条链」。如实带上这次调用真实的档位——嵌入两档同落
        // 一条泳道（ADR-083 ②「全映射」），所以它对选路不产生分歧，只影响那句话。
        let site = CallSite {
            capability: Capability::Embed,
            class: job.service_class(),
        };

        Box::pin(stream! {
            yield JobEvent::Started;
            if !breaker.health().is_ready() {
                yield JobEvent::Failed(JobError::ProviderUnavailable { provider: provider_id.clone() });
                return;
            }
            if cancel.is_cancelled() {
                yield JobEvent::Cancelled;
                return;
            }

            let url = format!("{base}{NATIVE_PATH}/embeddings");
            let body = serde_json::json!({ "profile": profile_id, "inputs": texts });

            // 401 → 刷新一次再试（同 `cloud_llm`：一批大文本可能跨过 access token 的有效期）。
            let mut token = token;
            let mut response = None;
            for attempt in 0..2u8 {
                if !gate.allows_outbound() {
                    yield JobEvent::Failed(JobError::StrictLocalBlocked {
                        capability: Capability::Embed,
                    });
                    return;
                }
                // 幂等键：同 `cloud_llm`，401 刷新后那一发是同一次操作的重发。
                let sent = crate::idempotency::tag(
                    http.post(&url).bearer_auth(&token).json(&body),
                    job_id,
                )
                .send()
                .await;
                let r = match sent {
                    Ok(r) => {
                        breaker.record_reachable();
                        r
                    }
                    Err(e) => {
                        let failure = if e.is_timeout() {
                            breaker.record_unreachable();
                            JobError::Timeout
                        } else if e.is_connect() {
                            breaker.record_unreachable();
                            JobError::ProviderUnavailable { provider: provider_id.clone() }
                        } else {
                            breaker.record_reachable();
                            JobError::Upstream {
                                provider: provider_id.clone(),
                                code: None,
                                message: e.to_string(),
                            }
                        };
                        yield JobEvent::Failed(failure);
                        return;
                    }
                };
                if r.status() != reqwest::StatusCode::UNAUTHORIZED || attempt == 1 {
                    response = Some(r);
                    break;
                }
                if !gate.allows_outbound() {
                    yield JobEvent::Failed(JobError::StrictLocalBlocked {
                        capability: Capability::Embed,
                    });
                    return;
                }
                // 传刚被拒的这一枚：几路 Provider 撞上同一次过期时，只有第一路真轮换，
                // 其余复用（见 `BearerSource::refresh`）。
                let Some(fresh) = bearer.refresh(&token).await else {
                    yield JobEvent::Failed(JobError::Unauthorized {
                        provider: provider_id.clone(),
                    });
                    return;
                };
                token = fresh;
            }
            // 从这里起每一个错误都是**这一枚**凭据换来的（同 `cloud_llm`）。
            let Some(response) = response else {
                yield JobEvent::Failed(JobError::Unauthorized {
                    provider: provider_id.clone(),
                });
                return;
            };

            let status = response.status();
            let retry_after = crate::llm_http::retry_after_secs(response.headers());
            let json: Value = response.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                let code = json.get("error").and_then(Value::as_str).unwrap_or("internal");
                let dimension = json
                    .get("dimension")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                yield JobEvent::Failed(map_server_error_with_retry(
                    &provider_id,
                    site,
                    code,
                    dimension,
                    retry_after,
                ));
                return;
            }
            // 取消可能在等响应期间到达：此刻丢弃结果比写一批用户已经不要的向量更对。
            if cancel.is_cancelled() {
                yield JobEvent::Cancelled;
                return;
            }

            // 执行者如实回报（PRD-17-033）：嵌入没有流，故随结果一起下发。
            if let Some(who) = executor_from(&json) {
                yield JobEvent::ExecutedBy(who);
            }
            let executed_model = json
                .pointer("/executed_by/model")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
                .map(ToOwned::to_owned);

            let Some(vectors) = parse_cloud_embeddings(&json, texts.len()) else {
                yield JobEvent::Failed(JobError::Upstream {
                    provider: provider_id.clone(),
                    code: None,
                    // **不含响应体**：那里面是向量（= 用户笔记正文的可攻击表示），
                    // 错误消息会进日志（TDD-12 脱敏）。只报"形状不对"这一事实。
                    message: format!("嵌入响应形状不符：期望 {} 条等长向量", texts.len()),
                });
                return;
            };

            yield JobEvent::Finished(JobResult {
                job_id,
                provider_id: provider_id.clone(),
                model_id: executed_model.as_deref().map(luna_core::ModelId::from),
                payload: ResultPayload::Embeddings(vectors),
                // 与 `/inference/v1/jobs` 的终态帧**同一份解析**：两条端点的 `usage`
                // 是同一个形状，客户端不该为此写两套（嵌入的 `tokens_out` 恒 0——
                // 它不产出 token，字段在，是为了让这一份解析吃两条端点）。
                usage: crate::cloud_llm::parse_usage(&json),
                timings: Timings::default(),
                output_truncated: None,
                finished_at: Timestamp::ZERO,
            });
        })
    }

    fn estimate(&self, _job: &Job) -> Option<UsageEstimate> {
        // 定价表随计费纵切；编一个数字会被用户当成账单预期。
        None
    }
}

fn once_failed(err: JobError) -> JobEventStream {
    Box::pin(stream! {
        yield JobEvent::Started;
        yield JobEvent::Failed(err);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt as _;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// 条数或维度对不上必须失败，不能"少几条也照收"。
    ///
    /// 向量要与「第 n 片」对位写进索引。静默少一条会让其后每一片都错位，而症状只是
    /// "这篇搜不太准"——一个不报错、也几乎无法归因的故障。维度不齐同样致命：半个索引
    /// 1024 维、半个 768 维，余弦从此没有意义。
    #[test]
    fn a_short_or_ragged_batch_is_rejected() {
        let two_expected = serde_json::json!({ "vectors": [[0.1, 0.2]] });
        assert!(
            parse_cloud_embeddings(&two_expected, 2).is_none(),
            "请求两片只回一条向量却被收下——索引会从此错位"
        );

        let ragged = serde_json::json!({ "vectors": [[0.1, 0.2], [0.3]] });
        assert!(
            parse_cloud_embeddings(&ragged, 2).is_none(),
            "维度不齐——余弦相似度从此没有意义"
        );

        let empty = serde_json::json!({ "vectors": [[]] });
        assert!(
            parse_cloud_embeddings(&empty, 1).is_none(),
            "空向量写进索引 = 那一片永远检索不到"
        );

        assert!(parse_cloud_embeddings(&serde_json::json!({}), 1).is_none());
    }

    #[test]
    fn a_well_formed_batch_parses() {
        let ok = serde_json::json!({ "vectors": [[0.1, 0.2], [0.3, 0.4]] });
        assert_eq!(
            parse_cloud_embeddings(&ok, 2),
            Some(vec![vec![0.1, 0.2], vec![0.3, 0.4]])
        );
    }

    /// 声明的能力与落点：只做嵌入，且 `location` 必须是 `Cloud`（严格本地据此过滤）。
    #[test]
    fn it_declares_embed_at_a_cloud_location() {
        struct NoBearer;
        #[async_trait]
        impl BearerSource for NoBearer {
            async fn bearer(&self) -> Option<String> {
                None
            }
            async fn refresh(&self, _rejected: &str) -> Option<String> {
                None
            }
        }
        let provider = CloudEmbedProvider::new(CloudEmbedConfig {
            api_base: "http://server.invalid".into(),
            display_name: "测试".into(),
            profile_id: "bge-m3:v1:1024:l2".to_owned(),
            bearer: Arc::new(NoBearer),
            gate: Arc::new(luna_core::AllowOutbound),
        });
        let caps = provider.capabilities();
        assert!(caps.supports(Capability::Embed));
        assert!(
            !caps.supports(Capability::Chat),
            "嵌入 Provider 声明了对话能力 = 路由会把对话派给它"
        );
        assert!(
            !caps.streaming,
            "嵌入没有增量，声明流式会让 UI 等一个永不到来的 partial"
        );
        assert_eq!(caps.location, ProviderLocation::Cloud);
        assert_eq!(
            provider.descriptor().model,
            None,
            "原生云端具体模型由服务端选择，客户端描述符不得固化模型名"
        );
    }

    #[tokio::test]
    async fn privacy_gate_is_rechecked_after_credential_await() {
        struct Gate(AtomicBool);
        impl luna_core::OutboundGate for Gate {
            fn allows_outbound(&self) -> bool {
                self.0.load(Ordering::SeqCst)
            }
        }
        struct ClosingBearer(Arc<Gate>);
        #[async_trait]
        impl BearerSource for ClosingBearer {
            async fn bearer(&self) -> Option<String> {
                self.0 .0.store(false, Ordering::SeqCst);
                Some("must-not-be-sent".to_owned())
            }
            async fn refresh(&self, _rejected: &str) -> Option<String> {
                panic!("隐私闸已关，不得刷新凭据")
            }
        }

        let gate = Arc::new(Gate(AtomicBool::new(true)));
        let provider = CloudEmbedProvider::new(CloudEmbedConfig {
            api_base: "http://127.0.0.1:1".to_owned(),
            display_name: "不得连接".to_owned(),
            profile_id: "bge-m3:v1:1024:l2".to_owned(),
            bearer: Arc::new(ClosingBearer(gate.clone())),
            gate,
        });
        let job = Job::new(
            JobKind::Embed {
                purpose: luna_core::EmbeddingPurpose::Document,
                texts: vec!["private".to_owned()],
            },
            luna_core::JobOrigin::Recording,
        );
        let events: Vec<_> = provider
            .execute(job, CancelToken::new())
            .await
            .collect()
            .await;
        assert!(matches!(events.first(), Some(JobEvent::Started)));
        assert!(matches!(
            events.last(),
            Some(JobEvent::Failed(JobError::StrictLocalBlocked {
                capability: Capability::Embed
            }))
        ));
    }
}
