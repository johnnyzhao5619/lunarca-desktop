//! 本地嵌入 Provider（ADR-027 / TDD-06 §3.5）。
//!
//! 与 [`crate::llm_http`] 同一个随包推理引擎、**另一条端点**（`/v1/embeddings`，ADR-143）。刻意不塞进
//! `LlmHttpProvider`：那个 Provider 的整条路径都围绕「合成 messages → 流式 token」构建，
//! 而嵌入既无 messages 也无流（一次请求一批向量），硬并进去等于给它加一条谁都不走的分支。
//!
//! # 三条硬约束（S16 本地嵌入 spike 与 ADR-143 P0，内部记录）
//!
//! 1. **超窗必须如实失败，绝不静默截断**。截断出来的向量维度照样正确，只代表前半段，
//!    表现出来只是"这篇搜不太准"——与蒸馏批次的上下文教训是同一个坑。随包引擎超窗即拒
//!    （归 [`JobError::InputTooLong`]），无需额外开关；换引擎时这条要重新验。
//! 2. **不加 `query:` / `passage:` 前缀**。bge-m3 不需要指令前缀，实测加了相似度反而下降
//!    （0.6364 < 0.6695）——那是纯噪音。
//! 3. **向量出厂即 L2 归一化**（实测范数 1.000000），故消费侧的余弦就是点积，此处**不再
//!    归一化**：多做一次除法只会引入浮点噪声，且会掩盖"换了个不归一化的模型"这件事。
//!
//! # 为什么不是 mlx-embeddings（2026-08-07 实测，M5 / 16 GB，bge-m3 200 片 × 1200 字符）
//!
//! 提过一个方向：本机嵌入改用 MLX 以吃满 Apple GPU。实测结论是**可行但不划算**，三条：
//!
//! 1. **更慢**。客户端实际用的 `EMBED_BATCH = 16` 档上，llama.cpp 路径约 13 片/秒，
//!    `mlx-embeddings` 只有 4.0；把 batch 放大到 100 给它最好条件也只到 7.2。
//!    llama.cpp 的 BERT 路径已经在 Metal 上，MLX 在这个尺寸的模型上没有余量可吃。
//! 2. **默认 API 用错了池化**。`outputs.text_embeds` 是 **mean-pooled**，而 bge-m3
//!    官方recipe 是 **CLS pooling**。后果是静默的检索质量下降：语义无关的两句话，
//!    正确池化下余弦 0.27~0.49，mean 池化下变成 0.58~0.75——什么都跟什么像。
//!    改取 `last_hidden_state[:, 0, :]` 再归一化后，与本路径的向量**逐条余弦 1.0000**，
//!    即权重没问题、错的是那个开箱即用的字段。
//! 3. **要引 Python sidecar**。该包是纯库、不带 HTTP server，接进来等于在
//!    ADR-010/013 的「ASR 进程内 FFI / LLM 进程外 HTTP，
//!    **不统一 sidecar**」之外再开第三种引擎形态，还要连 Python 运行时一起打包签名。
//!
//! 换实现即换 [`EmbeddingIndexIdentity`](../../luna-vault/src/index.rs)，全库重建——
//! 为一个更慢且默认更差的路径付这个代价不成立。有人再提时，先复现上面三个数字。
//!
//! 错误分类与 LLM 侧同源：`404` + `not found` → [`JobError::ModelMissing`]（不可重试，
//! 引导去装模型）；其余 → [`JobError::Upstream`]。缺模型时能力注册方**不注册**本 Provider，
//! 于是检索诚实降级为纯关键词（PRD-16-029）——绝不把一个 404 当成"库里没有相关内容"。

use async_stream::stream;
use async_trait::async_trait;
use luna_core::{
    AIProvider, CancelToken, Capability, CapabilitySet, Job, JobError, JobEvent, JobEventStream,
    JobKind, JobResult, ModelId, ProviderDescriptor, ProviderHealth, ProviderId, ProviderLocation,
    ResultPayload, Timestamp, Timings, Usage, UsageEstimate,
};
use serde_json::Value;
use std::sync::Arc;

use crate::llm_http::engine_base;
use crate::local_engine::{classify_engine_failure, LocalEngineSource, LOCAL_EMBED_PROVIDER_ID};

/// 连接建立超时（与 LLM 侧同口径：引擎失联应尽快归 `ProviderUnavailable`）。
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// 单批请求的整体超时。一批 16 片实测约 4 秒；给到 120 秒覆盖"模型冷启动 + 大批"的最坏
/// 情形，同时保证引擎僵死时任务不会永远挂着（读超时对流式不适用，但嵌入不是流式）。
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// 嵌入 Provider 配置。
#[derive(Clone)]
pub struct EmbedHttpConfig {
    /// 稳定 id。
    pub provider_id: ProviderId,
    /// 展示名。
    pub display_name: String,
    /// 随包引擎（地址与钥匙每次请求现问）。
    pub engine: Arc<dyn LocalEngineSource>,
    /// 模型名/标签（如 `bge-m3`；随包引擎为 preset 节名 = 目录条目 id）。
    pub model: String,
    /// 位置（决定严格本地过滤）。
    pub location: ProviderLocation,
}

impl EmbedHttpConfig {
    /// 随包推理引擎的嵌入端点（ADR-143）。向量出厂即 L2 归一化（P0 §6 实测范数 1.0），
    /// 消费侧的余弦即点积。
    #[must_use]
    pub fn local_engine(source: Arc<dyn LocalEngineSource>, model: impl Into<String>) -> Self {
        Self {
            provider_id: ProviderId::from(LOCAL_EMBED_PROVIDER_ID),
            display_name: "本机推理引擎（嵌入）".to_string(),
            engine: source,
            model: model.into(),
            location: ProviderLocation::LocalService,
        }
    }
}

/// 本地嵌入 Provider（非流式：一次请求 → 一批向量）。
pub struct EmbedHttpProvider {
    config: EmbedHttpConfig,
    client: reqwest::Client,
}

impl EmbedHttpProvider {
    /// 用配置构造。
    #[must_use]
    pub fn new(config: EmbedHttpConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("reqwest client 构造失败（TLS 后端初始化异常）"),
        }
    }

    /// 当前配置的模型名（供能力注册方与设置页回显真实状态）。
    #[must_use]
    pub fn model(&self) -> &str {
        &self.config.model
    }
}

/// 请求体（OpenAI 形）。随包引擎超窗即拒、从不截断，无需开关。
#[must_use]
fn build_request_body(model: &str, texts: &[String]) -> Value {
    serde_json::json!({
        "model": model,
        "input": texts,
        "encoding_format": "float",
    })
}

/// 端点 URL（`base` 取自 [`engine_base`]，含 `/v1`）。
fn endpoint_url(base: &str) -> String {
    format!("{}/embeddings", base.trim_end_matches('/'))
}

/// OpenAI 形响应（`{"data": [{"index", "embedding"}]}`）→ 按 `index` 排好的向量批。
/// 条数不符、下标不连续或维度不一致即 `None`——对不上号的向量比没有更糟。
#[must_use]
pub fn parse_embeddings(json: &str, expected: usize) -> Option<Vec<Vec<f32>>> {
    let value: Value = serde_json::from_str(json).ok()?;
    let mut rows: Vec<(u64, &Value)> = value
        .get("data")?
        .as_array()?
        .iter()
        .map(|row| Some((row.get("index")?.as_u64()?, row.get("embedding")?)))
        .collect::<Option<_>>()?;
    if rows.len() != expected {
        return None;
    }
    rows.sort_by_key(|(index, _)| *index);
    // 下标必须恰好是 0..n：缺一条或重一条都会让向量与分片错位。
    if rows
        .iter()
        .enumerate()
        .any(|(i, (index, _))| *index != i as u64)
    {
        return None;
    }
    let mut out: Vec<Vec<f32>> = Vec::with_capacity(rows.len());
    for (_, row) in rows {
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

/// 上游错误体里的 message（与 LLM 侧同一提取口径）。
fn upstream_message(body: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        let error = v.get("error")?;
        // 裸字符串 `{"error": "…"}` 与 OpenAI 形 `{"error": {"message": "…"}}` 都认。
        error
            .as_str()
            .or_else(|| error.get("message")?.as_str())
            .map(ToString::to_string)
    });
    let mut message = parsed.unwrap_or_else(|| body.to_string());
    message.truncate(500);
    message
}

/// 非 2xx → 结构化错误。`404` + `not found` = 模型未安装（可执行修复、不可重试）。
fn classify_failure(provider: &ProviderId, model: &str, status: u16, body: &str) -> JobError {
    let message = upstream_message(body);
    if status == 404 && message.contains("not found") {
        return JobError::ModelMissing {
            model: ModelId::from(model),
            hint: format!("嵌入模型 {model} 未安装：请在 设置 → 模型 中下载后重试"),
        };
    }
    JobError::Upstream {
        provider: provider.clone(),
        code: Some(status.to_string()),
        message,
    }
}

#[async_trait]
impl AIProvider for EmbedHttpProvider {
    fn id(&self) -> ProviderId {
        self.config.provider_id.clone()
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.config.provider_id.clone(),
            display_name: self.config.display_name.clone(),
            model: Some(self.config.model.clone()),
            location: self.config.location,
            license_summary: None,
        }
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet {
            capabilities: [Capability::Embed].into_iter().collect(),
            // 非流式：一次请求一批向量，没有"逐 token 到达"这回事。
            streaming: false,
            location: self.config.location,
            explicit_only: false,
        }
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::ready()
    }

    async fn execute(&self, job: Job, cancel: CancelToken) -> JobEventStream {
        let client = self.client.clone();
        let config = self.config.clone();
        let job_id = job.id;
        let texts = match &job.kind {
            JobKind::Embed { texts, .. } => texts.clone(),
            _ => Vec::new(),
        };

        Box::pin(stream! {
            yield JobEvent::Started;
            if texts.is_empty() {
                yield JobEvent::Failed(JobError::InvalidInput(
                    "嵌入 Provider 只接受 JobKind::Embed 且文本批不得为空".to_string(),
                ));
                return;
            }
            if cancel.is_cancelled() {
                yield JobEvent::Cancelled;
                return;
            }

            let Some((base, api_key)) = engine_base(config.engine.as_ref()) else {
                yield JobEvent::Failed(JobError::ProviderUnavailable {
                    provider: config.provider_id.clone(),
                });
                return;
            };
            let request = client
                .post(endpoint_url(&base))
                .json(&build_request_body(&config.model, &texts))
                .bearer_auth(&api_key);
            let response = match request.send().await {
                Ok(r) => r,
                Err(e) => {
                    yield JobEvent::Failed(if e.is_timeout() {
                        JobError::Timeout
                    } else if e.is_connect() {
                        JobError::ProviderUnavailable { provider: config.provider_id.clone() }
                    } else {
                        JobError::Upstream {
                            provider: config.provider_id.clone(),
                            code: e.status().map(|s| s.as_u16().to_string()),
                            message: e.to_string(),
                        }
                    });
                    return;
                }
            };

            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if !status.is_success() {
                let engine_failure =
                    classify_engine_failure(&config.provider_id, &config.model, status.as_u16(), &body);
                yield JobEvent::Failed(engine_failure.unwrap_or_else(|| {
                    classify_failure(&config.provider_id, &config.model, status.as_u16(), &body)
                }));
                return;
            }
            // 取消可能在等响应期间到达：此刻丢弃结果比写一批用户已经不要的向量更对。
            if cancel.is_cancelled() {
                yield JobEvent::Cancelled;
                return;
            }
            let Some(vectors) = parse_embeddings(&body, texts.len()) else {
                yield JobEvent::Failed(JobError::Upstream {
                    provider: config.provider_id.clone(),
                    code: None,
                    // **不含响应体**：那里面是向量（= 用户笔记正文的可攻击表示），
                    // 错误消息会进日志（TDD-12 脱敏）。只报"形状不对"这一事实。
                    message: format!(
                        "嵌入响应形状不符：期望 {} 条等长向量",
                        texts.len()
                    ),
                });
                return;
            };

            yield JobEvent::Finished(JobResult {
                job_id,
                provider_id: config.provider_id.clone(),
                model_id: Some(ModelId::from(config.model.as_str())),
                payload: ResultPayload::Embeddings(vectors),
                usage: Usage::default(),
                timings: Timings::default(),
                output_truncated: None,
                finished_at: Timestamp::ZERO,
            });
        })
    }

    fn estimate(&self, _job: &Job) -> Option<UsageEstimate> {
        // 本地模型零费用；耗时估算意义不大（S16：与总 token 数近线性，调用方自己按片数估）。
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_body_passes_input_verbatim() {
        let body = build_request_body("bge-m3", &["甲".to_string(), "乙".to_string()]);
        assert_eq!(body["model"], "bge-m3");
        assert_eq!(body["input"], serde_json::json!(["甲", "乙"]));
        // 不加 query:/passage: 前缀（实测加了相似度反降）——输入必须原样。
        assert!(!body["input"].to_string().contains("query"));
        assert_eq!(
            endpoint_url("http://127.0.0.1:1/v1/"),
            "http://127.0.0.1:1/v1/embeddings"
        );
    }

    #[test]
    fn parses_batch_by_index_and_rejects_mismatched_shapes() {
        let shuffled =
            r#"{"data":[{"index":1,"embedding":[0.3,0.4]},{"index":0,"embedding":[0.1,0.2]}]}"#;
        assert_eq!(
            parse_embeddings(shuffled, 2),
            Some(vec![vec![0.1, 0.2], vec![0.3, 0.4]])
        );
        // 条数不符即拒绝：向量与分片是按下标对应的，少一条就整批错位——
        // 那会让每一篇的语义都指向别人的内容，且没有任何征兆。
        assert_eq!(parse_embeddings(shuffled, 3), None);
        // 缺下标 / 重下标：哪怕条数对得上也拒。
        let gap = r#"{"data":[{"index":0,"embedding":[0.1]},{"index":2,"embedding":[0.2]}]}"#;
        assert_eq!(parse_embeddings(gap, 2), None);
        let dup = r#"{"data":[{"index":0,"embedding":[0.1]},{"index":0,"embedding":[0.2]}]}"#;
        assert_eq!(parse_embeddings(dup, 2), None);
        // 维度不齐 / 空向量同理拒绝。
        let ragged =
            r#"{"data":[{"index":0,"embedding":[0.1,0.2]},{"index":1,"embedding":[0.3]}]}"#;
        assert_eq!(parse_embeddings(ragged, 2), None);
        assert_eq!(
            parse_embeddings(r#"{"data":[{"index":0,"embedding":[]}]}"#, 1),
            None
        );
        assert_eq!(parse_embeddings(r#"{"data":[]}"#, 1), None);
        assert_eq!(parse_embeddings(r#"{"other":1}"#, 1), None);
        assert_eq!(parse_embeddings("not json", 1), None);
    }

    #[test]
    fn missing_model_is_actionable_not_generic_upstream() {
        let provider = ProviderId::from(LOCAL_EMBED_PROVIDER_ID);
        let err = classify_failure(
            &provider,
            "bge-m3",
            404,
            r#"{"error":"model \"bge-m3\" not found, try pulling it first"}"#,
        );
        match err {
            JobError::ModelMissing { model, hint } => {
                assert_eq!(model.as_str(), "bge-m3");
                assert!(hint.contains("设置 → 模型"), "提示必须可执行：{hint}");
            }
            other => panic!("缺模型必须归 ModelMissing（不可重试 + 可执行修复）：{other:?}"),
        }
        // 其余状态码保留上游消息（5xx/限流可重试）。
        assert!(matches!(
            classify_failure(&provider, "bge-m3", 500, r#"{"error":"boom"}"#),
            JobError::Upstream { .. }
        ));
    }
}
