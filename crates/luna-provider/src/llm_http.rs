//! 随包推理引擎（`llama-server`，ADR-143）的 HTTP 流式 LLM Provider（TDD-01 §3.2、TDD-03 §3）。
//!
//! 请求打到引擎的 `/v1/chat/completions`（SSE 流），发它认的扩展字段（关思考、要用量、输出
//! 预算）；地址与钥匙每次请求现问（[`LocalEngineSource`]，引擎进程可能刚换了端口）。云端
//! 推理走原生推理面（[`crate::cloud_llm`]），不经这里。
//!
//! **错误分类**（按「用户能不能据此行动」划线）：非 2xx 响应解析上游 JSON 错误体——
//! `404` + "not found" 归 [`JobError::ModelMissing`]（去装或换模型，不可重试）；
//! `429` 归 [`JobError::RateLimited`]（遵从 `Retry-After`）；其余归 [`JobError::Upstream`]
//!（保留上游消息，5xx 可重试）。随包引擎特有的错误形状先经 `local_engine` 归一。

use crate::local_engine::{
    classify_engine_failure, LocalEngineSource, LOCAL_LLM_PROVIDER_ID,
    LOCAL_LLM_REALTIME_PROVIDER_ID,
};
use crate::sse::{SseDecoder, SsePayload};
use crate::{HeuristicTokenCounter, TokenCount, TokenCounter};
use async_stream::stream;
use async_trait::async_trait;
use futures::StreamExt;
use luna_core::{
    AIProvider, CancelToken, Capability, CapabilitySet, ChatRole, Job, JobError, JobEvent,
    JobEventStream, JobKind, JobOptions, JobResult, ModelId, PartialPayload, PromptRegistry,
    PromptScope, ProviderDescriptor, ProviderHealth, ProviderId, ProviderLocation, ResultPayload,
    TextRef, Timestamp, Timings, Usage, UsageEstimate,
};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

/// 一条 OpenAI 风格消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenAiMessage {
    /// `system` / `user` / `assistant`。
    pub role: String,
    /// 内容。
    pub content: String,
}

fn sys(content: impl Into<String>) -> OpenAiMessage {
    OpenAiMessage {
        role: "system".into(),
        content: content.into(),
    }
}
fn user(content: impl Into<String>) -> OpenAiMessage {
    OpenAiMessage {
        role: "user".into(),
        content: content.into(),
    }
}

fn text_of(t: &TextRef) -> String {
    match t {
        TextRef::Inline(s) => s.clone(),
    }
}

fn role_str(r: ChatRole) -> &'static str {
    match r {
        ChatRole::System => "system",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
    }
}

/// 一次合成的产物：消息 + 本次用的提示词版本。
///
/// 版本与消息**同时产出**，而不是让调用方事后再去问一次注册表：目录可能在两次
/// 调用之间刷新，那时上报的版本就不是真正合成这批消息的那一版——一个永远对不上、
/// 又永远查不出来的账。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledPrompt {
    pub messages: Vec<OpenAiMessage>,
    /// 见 `luna_core::prompt`：`0` = 随包默认，≥1 = 已验签提示词目录版本。
    pub prompt_version: u32,
    /// 这批消息用的是哪一份灰度变体（ADR-050 修订 ④）；`None` = 对照组或随包默认。
    pub prompt_variant: Option<String>,
}

/// 把 Job 合成为 OpenAI chat messages。
///
/// **这是全客户端唯一的能力提示词组装点**（ADR-050）。模板一律来自
/// [`PromptRegistry`]，本函数只负责：决定用哪几条模板、准备变量、拼消息。
/// 空 `messages` = 该 Job 类型不适用本 Provider。
#[must_use]
pub fn build_chat_messages(registry: &PromptRegistry, job: &Job) -> AssembledPrompt {
    // 版本与变体一次取齐（见 `PromptRegistry::stamp`）。
    let luna_core::PromptStamp {
        version: prompt_version,
        variant: prompt_variant,
    } = registry.stamp();
    // 结构保真约束（服务"替换原文"，TDD-03 §3.3）：翻译/润色都要保留原行/段/列表结构，
    // 故它是共享片段而不是两份措辞必须同步的字面量。
    let structure = registry.render(PromptScope::Common, "structure", &[]);
    let messages = match &job.kind {
        JobKind::Translate {
            text,
            source,
            target,
            instruction,
            ..
        } => {
            // `instruction`（合成后的风格/场景，TDD-03 §3.3）有则附加，无则用通用译文 prompt。
            let style = match instruction {
                Some(s) if !s.is_empty() => registry.render(
                    PromptScope::Translate,
                    "style",
                    &[("instruction", s.as_str())],
                ),
                _ => String::new(),
            };
            // 语言全称（非 BCP-47 代码）：小模型对「into English」远比「into en」敏感，
            // 显式声明源语言并强制「整段输出必须为目标语言」，根治退化为源语改写（TDD-03 §3.3）。
            let source_hint = source
                .as_ref()
                .map(|s| {
                    registry.render(
                        PromptScope::Translate,
                        "source_hint",
                        &[("source_lang", s.display_name())],
                    )
                })
                .unwrap_or_default();
            vec![
                sys(registry.render(
                    PromptScope::Translate,
                    "system",
                    &[
                        ("target_lang", target.display_name()),
                        ("source_hint", &source_hint),
                        ("style", &style),
                        ("structure", &structure),
                    ],
                )),
                user(registry.render(PromptScope::Translate, "user", &[("text", &text_of(text))])),
            ]
        }
        JobKind::Refine { text, instruction } => {
            let rule = instruction.as_ref().map_or_else(
                || registry.render(PromptScope::Refine, "default_rule", &[]),
                |i| i.as_str().to_owned(),
            );
            vec![
                sys(registry.render(
                    PromptScope::Refine,
                    "system",
                    &[("rule", &rule), ("structure", &structure)],
                )),
                user(text_of(text)),
            ]
        }
        JobKind::Summarize {
            text,
            style,
            instruction,
        } => {
            let instruction = instruction
                .as_ref()
                .map(|i| i.as_str())
                .filter(|value| !value.is_empty())
                .unwrap_or("");
            // `Faithful` 档：`instruction` **即完整 system prompt**，绝不外套
            // 「Summarize … concisely / Output only the summary」——蒸馏的「整理稿」要求的是
            // 零压缩的保真改写，套上这层前缀就是给模型两条相反的指令，本地小模型多半听
            // 外层的，于是整理稿被静默做成另一份摘要。与 `ExtractTags`「调用方决策、
            // 引擎只渲染」同一先例。
            //
            // 其余档**维持前缀**：悬浮菜单等调用方传的是 `"Write the summary in zh."` 这类
            // **半句片段**，靠前缀撑起语义——"有 instruction 就整句替换"的一刀切会砸到它们。
            if style.uses_raw_instruction() && !instruction.is_empty() {
                vec![sys(instruction.to_owned()), user(text_of(text))]
            } else {
                vec![
                    sys(registry.render(
                        PromptScope::Summarize,
                        "system",
                        &[("instruction", instruction)],
                    )),
                    user(text_of(text)),
                ]
            }
        }
        JobKind::Chat { messages, .. } => messages
            .iter()
            .map(|m| OpenAiMessage {
                role: role_str(m.role).to_string(),
                content: m.content.clone(),
            })
            .collect(),
        JobKind::ExtractTags { text, instruction } => {
            // 指令 = 完整合成的 system prompt（含既有词表与 JSON 输出契约，TDD-03 §3.3
            // 同哲学：调用方决策、引擎只渲染）。
            vec![sys(instruction.as_str()), user(text_of(text))]
        }
        JobKind::Custom { vars, .. } => {
            // Phase 2：把变量平铺为一条 user 消息；模板渲染在 Phase 3 PromptPreset。
            let body = vars
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join("\n");
            vec![user(body)]
        }
        // 本 Provider 不做 ASR（capabilities 不含 Transcribe，Router 不会路由到此）。
        // 非文本 LLM 能力（转写/OCR/细分）不适用本 Provider（各自路由到 whisper / vision-ocr / sherpa-diarize）。
        // 嵌入虽然输入也是文本，但它**不是对话形态**（没有 messages 这回事，端点也不同）：
        // 归 `EmbedHttpProvider`（同一个引擎、另一条端点，ADR-027）。
        JobKind::Transcribe { .. }
        | JobKind::Ocr { .. }
        | JobKind::Diarize { .. }
        | JobKind::Embed { .. } => Vec::new(),
    };
    AssembledPrompt {
        messages,
        prompt_version,
        prompt_variant,
    }
}

/// 从一个 OpenAI 流式 chunk 的 JSON 里取出本次 `delta.content`（无内容返回 `None`）。
#[must_use]
pub fn parse_delta(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value
        .get("choices")?
        .get(0)?
        .get("delta")?
        .get("content")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

/// 从一个 OpenAI 流式 chunk 里取出对方**自报**的 token 用量（`prompt_tokens` /
/// `completion_tokens`）。没有这个字段返回 `None`。
///
/// # 为什么是「解析自报」而不是「客户端估算」
///
/// [`luna_core::Usage`] 的字段是 `Option`，**`None` 本身就表示「不知道」**。用一个按
/// 字符数猜出来的数字填上，是把未知伪装成已知——而它会和本机档那些**实测**值
/// （随包引擎流末自报的用量）落进「活动 · 用量」的同一列，从此没人分得清
/// 哪个数字可以拿去对账。这正是用量计量里最常见的一类事故：表上一个数、账单上另一个数，
/// 而没有任何地方记录过它们的来源不同。
///
/// 故本函数只取对方给的真实数字。**不发 `stream_options.include_usage` 去索要**：
/// 那是标准字段没错，但对面是用户自填的任意 OpenAI 兼容服务，多一个字段就多一种
/// 400 的可能——为一个用量数字去赌用户的翻译能不能用，不划算。对方肯给就记，不给就
/// 如实留空。
#[must_use]
pub fn parse_openai_usage(json: &str) -> Option<(Option<u32>, Option<u32>)> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let usage = value.get("usage")?;
    let tokens_in = count_field(usage, "prompt_tokens");
    let tokens_out = count_field(usage, "completion_tokens");
    (tokens_in.is_some() || tokens_out.is_some()).then_some((tokens_in, tokens_out))
}

/// 随包引擎流里一帧除正文 / 用量之外的事实。
#[derive(Debug, Clone, PartialEq, Eq)]
enum EngineFrame {
    /// 流中错误帧（原样交给错误分类）。
    Error(String),
    /// 带 `finish_reason` 的收尾帧；`truncated` = 输出预算触顶（`length`）。
    Finished { truncated: bool },
    /// 其余。
    Other,
}

fn parse_engine_frame(json: &str) -> EngineFrame {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return EngineFrame::Other;
    };
    if value.get("error").is_some() {
        return EngineFrame::Error(json.to_owned());
    }
    match value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("finish_reason"))
        .and_then(Value::as_str)
    {
        Some(reason) => EngineFrame::Finished {
            truncated: reason == "length",
        },
        None => EngineFrame::Other,
    }
}

/// 取一个非负整数计数字段；缺失、类型不符或溢出 `u32` 一律当作"没有"。
///
/// 宁可报 `None` 也不截断：一个被 `as u32` 截断的用量数字比没有数字更糟——它看起来
/// 是真的，却在对账时对不上，而且没有任何地方会因此报错。
fn count_field(value: &Value, key: &str) -> Option<u32> {
    u32::try_from(value.get(key)?.as_u64()?).ok()
}

#[must_use]
fn build_request_body(
    config: &LlmHttpConfig,
    messages: &[OpenAiMessage],
    options: &JobOptions,
) -> Value {
    // 上下文窗**不能**逐请求给：它在引擎加载时由内存预检定下（preset `ctx-size`），超窗即被
    // 如实拒绝。
    {
        {
            let mut body = serde_json::json!({
                "model": config.model,
                "messages": messages,
                "stream": true,
                "stream_options": { "include_usage": true },
                // Qwen3 / 3.5 的模板默认先想后答：不关的话正文为空、全进 `reasoning_content`
                // （P3 预探实测：0.8B 吐了一句 "Okay," 就因预算耗尽收尾）。不认这个参数的
                // 模板会忽略它。
                "chat_template_kwargs": { "enable_thinking": false },
            });
            if let Some(budget) = options.output_budget {
                body["max_tokens"] = budget.into();
            }
            body
        }
    }
}

/// 引擎的流式对话端点 URL（`base` 已含 `/v1`）。
#[must_use]
fn endpoint_url(base: &str) -> String {
    format!("{}/chat/completions", base.trim_end_matches('/'))
}

/// 从上游错误体提取人类可读消息（兼容两种形态：OpenAI `{"error":{"message":..}}` 与
/// 裸字符串 `{"error":"..."}`）；解析失败回退截断原文。
#[must_use]
fn upstream_error_message(body: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body).ok().and_then(|value| {
        let err = value.get("error")?;
        err.as_str()
            .map(ToString::to_string)
            .or_else(|| err.get("message")?.as_str().map(ToString::to_string))
    });
    let mut message = parsed.unwrap_or_else(|| body.to_string());
    message.truncate(500);
    message
}

/// 把随包引擎的非 2xx 响应归类为结构化错误。
///
/// 分类的意义在于**用户能不能据此行动**：
/// - `404` + "not found" → 模型未安装（去装或换模型，不可重试）；
/// - `429` → 限流（带 `Retry-After` 时按它退避）；
/// - 其余保留上游消息（5xx 可重试）。
///
/// 本通道只连随包引擎（ADR-155）：钥匙是进程内生成的，没有用户凭据会被拒，也没有额度
/// 可言——401/402 不在这里分类，落到 `Upstream` 如实上报。
#[must_use]
#[cfg(test)]
fn classify_http_failure(provider: &ProviderId, model: &str, status: u16, body: &str) -> JobError {
    classify_http_failure_with_retry(provider, model, status, body, None)
}

fn classify_http_failure_with_retry(
    provider: &ProviderId,
    model: &str,
    status: u16,
    body: &str,
    retry_after_secs: Option<u64>,
) -> JobError {
    let message = upstream_error_message(body);
    if status == 404 && message.contains("not found") {
        return JobError::ModelMissing {
            model: ModelId::from(model),
            hint: format!("模型 {model} 未安装：请在 设置 → 模型 中安装，或切换默认模型"),
        };
    }
    if status == 429 {
        return JobError::RateLimited {
            provider: provider.clone(),
            retry_after_secs,
        };
    }
    JobError::Upstream {
        provider: provider.clone(),
        code: Some(status.to_string()),
        message,
    }
}

/// RFC 9110 `Retry-After` → 从现在起至少等待多少秒。
///
/// 两种合法 wire 形状都接受：整数秒与 HTTP-date。日期已过时归 0；非法值如实归 `None`，
/// 由调用方的既有退避策略兜底，不编一个等待时长。
fn retry_after_secs_at(
    headers: &reqwest::header::HeaderMap,
    now: std::time::SystemTime,
) -> Option<u64> {
    let raw = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(seconds) = raw.parse::<u64>() {
        return Some(seconds);
    }
    let at = httpdate::parse_http_date(raw).ok()?;
    Some(at.duration_since(now).unwrap_or_default().as_secs())
}

pub(crate) fn retry_after_secs(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    retry_after_secs_at(headers, std::time::SystemTime::now())
}

/// LLM HTTP Provider 配置。
#[derive(Clone)]
pub struct LlmHttpConfig {
    /// 稳定 id（默认档 / 实时档各一）。
    pub provider_id: ProviderId,
    /// 展示名。
    pub display_name: String,
    /// 随包引擎（地址与钥匙每次请求现问）。
    pub engine: Arc<dyn LocalEngineSource>,
    /// preset 节名 = 目录条目 id。
    pub model: String,
    /// 仅接受任务显式定向；实时档为 true，默认档为 false。
    pub explicit_only: bool,
}

/// 引擎此刻的 `(基址含 /v1, 钥匙)`；没起、正在起、熔断时为 `None`（= 此刻连不上）。
/// 对话与嵌入共用这一处。
#[must_use]
pub fn engine_base(source: &dyn LocalEngineSource) -> Option<(String, String)> {
    source.endpoint().map(|e| {
        (
            format!("{}/v1", e.base_url.trim_end_matches('/')),
            e.api_key,
        )
    })
}

impl LlmHttpConfig {
    /// 随包推理引擎**默认 / 质量档**（ADR-143）。`model` 是 preset 节名（目录条目 id）。
    #[must_use]
    pub fn local_engine(source: Arc<dyn LocalEngineSource>, model: impl Into<String>) -> Self {
        Self::local_engine_with_id(LOCAL_LLM_PROVIDER_ID, source, model)
    }

    /// 随包推理引擎**实时档**：只承接实时字幕 / 录音翻译等翻译的显式定向。
    #[must_use]
    pub fn local_engine_realtime(
        source: Arc<dyn LocalEngineSource>,
        model: impl Into<String>,
    ) -> Self {
        Self::local_engine_with_id(LOCAL_LLM_REALTIME_PROVIDER_ID, source, model)
    }

    fn local_engine_with_id(
        id: &str,
        source: Arc<dyn LocalEngineSource>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            provider_id: ProviderId::from(id),
            display_name: "本机推理引擎".into(),
            engine: source,
            model: model.into(),
            explicit_only: id == LOCAL_LLM_REALTIME_PROVIDER_ID,
        }
    }
}

/// OpenAI 兼容 LLM Provider（流式）。
pub struct LlmHttpProvider {
    config: LlmHttpConfig,
    client: reqwest::Client,
    /// 失败计数熔断（TDD-01 §3.1）。`Arc` 是因为 `execute` 把它移进返回的流里。
    breaker: std::sync::Arc<crate::breaker::Breaker>,
}

/// 连接建立超时：引擎失联应尽快归 `ProviderUnavailable`（读超时不设——流式长生成合法）。
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl LlmHttpProvider {
    /// 用配置构造。
    #[must_use]
    pub fn new(config: LlmHttpConfig) -> Self {
        Self {
            config,
            breaker: std::sync::Arc::new(crate::breaker::Breaker::new()),
            client: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .build()
                .expect("reqwest client 构造失败（TLS 后端初始化异常）"),
        }
    }
}

impl TokenCounter for LlmHttpProvider {
    fn count_chat_tokens(&self, messages: &[luna_core::ChatMessage]) -> TokenCount {
        // Phase 5 MVP：未登记 tokenizer 的模型走保守启发式；精确 tokenizer 后续替换同一接缝。
        HeuristicTokenCounter::new().count_chat_tokens(messages)
    }
}

fn net_err(provider: &ProviderId, e: &reqwest::Error) -> JobError {
    if e.is_timeout() {
        JobError::Timeout
    } else if e.is_connect() {
        JobError::ProviderUnavailable {
            provider: provider.clone(),
        }
    } else {
        JobError::Upstream {
            provider: provider.clone(),
            code: e.status().map(|s| s.as_u16().to_string()),
            message: e.to_string(),
        }
    }
}

#[async_trait]
impl AIProvider for LlmHttpProvider {
    fn id(&self) -> ProviderId {
        self.config.provider_id.clone()
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.config.provider_id.clone(),
            display_name: self.config.display_name.clone(),
            model: Some(self.config.model.clone()),
            location: ProviderLocation::LocalService,
            license_summary: None,
        }
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet {
            capabilities: [
                Capability::Translate,
                Capability::Refine,
                Capability::Summarize,
                Capability::Chat,
                Capability::Custom,
            ]
            .into_iter()
            .collect(),
            streaming: true,
            location: ProviderLocation::LocalService,
            explicit_only: self.config.explicit_only,
        }
    }

    async fn health(&self) -> ProviderHealth {
        // **不主动探活**——那对云端点意味着一次自发的云网络行为，会当场推翻 ADR-037 的
        // 运行期不变量，且没有任何用户动作能解释那次请求。可用性由真实请求暴露，连不上
        // 累计到阈值才拒绝派活（`breaker` 顶注写明了「什么算失败」的边界）。
        self.breaker.health()
    }

    async fn execute(&self, job: Job, cancel: CancelToken) -> JobEventStream {
        let client = self.client.clone();
        let config = self.config.clone();
        let breaker = std::sync::Arc::clone(&self.breaker);
        let job_id = job.id;
        // 提示词目录是进程级产品配置（`luna_core::prompt::registry`）：本机这一档没有把版本
        // 上报到任何账本的去处（对面不是我们的服务端），故只取消息。
        let messages = build_chat_messages(luna_core::prompt::registry(), &job).messages;
        let options = job.options.clone();

        Box::pin(stream! {
            yield JobEvent::Started;

            // 熔断打开时**不再去连**——这就是熔断存在的意义，也是它今天唯一的消费者。
            //
            // 端点挂掉时，此前每一次翻译 / 对话都要各等一次连接超时（5 s）才失败；现在连续
            // 三次连不上之后的一个退避周期内立即失败。给用户的结论与真实不可达完全一致
            // （端点确实连不上），修复动作也一致，故**不为「还有 N 秒自动重试」改 wire**：
            // 那是内部退避细节，用户重试一次即可，不值得动一个跨前后端的错误契约。
            if !breaker.health().is_ready() {
                yield JobEvent::Failed(JobError::ProviderUnavailable {
                    provider: config.provider_id.clone(),
                });
                return;
            }

            if messages.is_empty() {
                yield JobEvent::Failed(JobError::InvalidInput(
                    "LLM provider 不支持该 Job 类型".to_string(),
                ));
                return;
            }

            // 随包引擎没就绪（没起、正在起、熔断）= 此刻连不上；不计入熔断——这一刻
            // 根本没有发出任何字节，引擎自己的监管层已在处理它。
            let Some((base, api_key)) = engine_base(config.engine.as_ref()) else {
                yield JobEvent::Failed(JobError::ProviderUnavailable {
                    provider: config.provider_id.clone(),
                });
                return;
            };
            let url = endpoint_url(&base);
            let body = build_request_body(&config, &messages, &options);
            let request = client.post(&url).json(&body).bearer_auth(&api_key);
            let response = match request.send().await {
                Ok(r) => {
                    // 抵达了端点——哪怕它随后返回 4xx/5xx，端点本身是活的。
                    breaker.record_reachable();
                    r
                }
                Err(e) => {
                    let failure = net_err(&config.provider_id, &e);
                    // 只有「没能抵达」才计入熔断：额度用尽 / 未登录 / 模型不存在都走
                    // `classify_http_failure`，那些恰恰证明端点是活的，且各自有用户必须
                    // 知道且能处理的动作——计入熔断会让他修好之后还得莫名再等一个退避周期。
                    if matches!(failure, JobError::Timeout | JobError::ProviderUnavailable { .. }) {
                        breaker.record_unreachable();
                    } else {
                        breaker.record_reachable();
                    }
                    yield JobEvent::Failed(failure);
                    return;
                }
            };

            if !response.status().is_success() {
                let status = response.status().as_u16();
                let retry_after = retry_after_secs(response.headers());
                let body = response.text().await.unwrap_or_default();
                let engine_failure =
                    classify_engine_failure(&config.provider_id, &config.model, status, &body);
                yield JobEvent::Failed(engine_failure.unwrap_or_else(|| {
                    classify_http_failure_with_retry(
                        &config.provider_id,
                        &config.model,
                        status,
                        &body,
                        retry_after,
                    )
                }));
                return;
            }

            let mut sse = SseDecoder::new();
            let mut accumulated = String::new();
            // 用量：只记**对方自报的实测值**（OpenAI 兼容流的 `usage` 帧；随包引擎靠
            // `stream_options.include_usage` 必给）。对方不给就留空——`Option` 本就表示「不知道」，
            // 而按字符数猜一个填进去会和实测值混在同一列里，从此分不清哪个能拿去对账。
            // 客户端估算已在评估后否决，理由见 `parse_openai_usage` 与 TDD-14 §3 修订注。
            let mut usage = Usage::default();
            // 只有随包引擎这一档如实给出 `finish_reason`：`length` = 输出预算触顶。
            let mut output_truncated = None;
            let mut bytes = response.bytes_stream();

            'outer: loop {
                if cancel.is_cancelled() {
                    yield JobEvent::Cancelled;
                    return;
                }
                let chunk = tokio::select! {
                    chunk = bytes.next() => chunk,
                    () = tokio::time::sleep(std::time::Duration::from_millis(50)) => continue,
                };
                let Some(chunk) = chunk else {
                    break;
                };
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        yield JobEvent::Failed(net_err(&config.provider_id, &e));
                        return;
                    }
                };
                let text = String::from_utf8_lossy(&chunk);
                {
                    {
                        for payload in sse.push(&text) {
                            match payload {
                                SsePayload::Done => break 'outer,
                                // OpenAI 兼容流一律匿名 `data:`，故不看事件名。
                                SsePayload::Data { json, .. } => {
                                    {
                                        match parse_engine_frame(&json) {
                                            EngineFrame::Error(failure) => {
                                                yield JobEvent::Failed(
                                                    classify_engine_failure(
                                                        &config.provider_id,
                                                        &config.model,
                                                        500,
                                                        &failure,
                                                    )
                                                    .unwrap_or_else(|| JobError::Upstream {
                                                        provider: config.provider_id.clone(),
                                                        code: None,
                                                        message: upstream_error_message(&failure),
                                                    }),
                                                );
                                                return;
                                            }
                                            // 给了 `max_tokens` 时触顶的是调用方的额度；没给时
                                            // 是引擎加载窗用完了——那只能换模型或缩短输入。
                                            EngineFrame::Finished { truncated: true } => {
                                                output_truncated = Some(if options.output_budget.is_some() {
                                                    luna_core::TruncatedBy::Caller
                                                } else {
                                                    luna_core::TruncatedBy::Upstream
                                                });
                                            }
                                            EngineFrame::Finished { truncated: false } => {}
                                            EngineFrame::Other => {}
                                        }
                                    }
                                    // 用量帧与内容帧不互斥：带 `usage` 的那一帧通常
                                    // `choices` 为空，但没有哪条规矩禁止两者同帧，故都试。
                                    if let Some((tokens_in, tokens_out)) = parse_openai_usage(&json) {
                                        usage = Usage { tokens_in, tokens_out, ..Usage::default() };
                                    }
                                    if let Some(token) = parse_delta(&json) {
                                        accumulated.push_str(&token);
                                        yield JobEvent::Partial(PartialPayload::Token(token));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            yield JobEvent::Finished(JobResult {
                job_id,
                provider_id: config.provider_id.clone(),
                model_id: Some(ModelId::from(config.model.as_str())),
                payload: ResultPayload::Text(accumulated),
                usage,
                timings: Timings::default(),
                output_truncated,
                finished_at: Timestamp::ZERO,
            });
        })
    }

    fn estimate(&self, _job: &Job) -> Option<UsageEstimate> {
        None // Phase 2：费用估算接 TokenCounter + 定价表后实现
    }
}

#[cfg(test)]
mod tests {

    /// 熔断打开时**必须短路**——不短路等于这个熔断器不存在。
    ///
    /// 端点挂掉时，此前每一次翻译 / 对话都各等一次连接超时（5 s）；这条钉住的是「连续
    /// 三次连不上之后立即失败」。用一个必然连不上的地址走真实路径，前三次各等一次超时，
    /// 第四次必须**立刻**回来。
    #[tokio::test]
    async fn an_open_breaker_fails_fast_instead_of_dialing_again() {
        use luna_core::CancelToken;

        // 本机回环 + 无人监听的端口：内核立刻回 ECONNREFUSED。刻意不用 TEST-NET 那类
        // 黑洞地址——那会让这条用例真的等满三次连接超时（实测 16 s），把一条本该毫秒级的
        // 断言变成 CI 里最慢的那个。
        let provider = LlmHttpProvider::new(LlmHttpConfig::local_engine(
            Arc::new(FixedEngine("http://127.0.0.1:1")),
            "m",
        ));
        // 用 Translate 而不是空 Chat：空消息会在发请求**之前**被判 InvalidInput，
        // 那样这条用例根本走不到网络，熔断自然不会动——一条永远绿的假断言。
        let job = || {
            Job::new(
                JobKind::Translate {
                    text: TextRef::Inline("hello".into()),
                    source: None,
                    target: Lang::from("zh"),
                    instruction: None,
                    template: None,
                },
                JobOrigin::PopupTranslate,
            )
        };

        for _ in 0..3 {
            let mut stream = provider.execute(job(), CancelToken::new()).await;
            while stream.next().await.is_some() {}
        }
        assert!(
            !provider.health().await.is_ready(),
            "三次连不上之后应已熔断"
        );

        let started = std::time::Instant::now();
        let mut stream = provider.execute(job(), CancelToken::new()).await;
        let mut failed = false;
        while let Some(event) = stream.next().await {
            if matches!(
                event,
                JobEvent::Failed(JobError::ProviderUnavailable { .. })
            ) {
                failed = true;
            }
        }
        assert!(failed, "熔断态下应以 ProviderUnavailable 收场");
        assert!(
            started.elapsed() < CONNECT_TIMEOUT,
            "熔断打开后仍在拨号——用户每次都白等一次连接超时"
        );
    }

    /// **只有「没能抵达端点」才计入熔断**。
    ///
    /// 引擎明确回答了的那些——限流、模型不存在、上游 5xx——统统证明端点
    /// 是活的，且各自有用户必须知道且能处理的动作。把它们计入熔断的后果是：用户加完额度
    /// 回来，还要莫名再等一个退避周期，而这段时间里他得到的错误是「服务不可用」，
    /// 指向一个完全错误的修复方向。
    #[test]
    fn only_unreachability_counts_toward_the_breaker() {
        let provider = ProviderId::from(LOCAL_LLM_PROVIDER_ID);
        // 端点答复了（HTTP 层面成功抵达）的三种失败，都不该被当成不可达。
        for failure in [
            classify_http_failure(&provider, "m", 429, "busy"),
            classify_http_failure(&provider, "m", 404, "model not found"),
            classify_http_failure(&provider, "m", 500, "upstream boom"),
        ] {
            assert!(
                !matches!(
                    failure,
                    JobError::Timeout | JobError::ProviderUnavailable { .. }
                ),
                "{failure:?} 会被计入熔断——端点其实是活的，用户修好后还得再等一个退避周期"
            );
        }
    }

    use super::*;

    use luna_core::{JobOrigin, Lang, SummaryStyle};

    /// 组装用例一律各建各的随包注册表——**不碰进程级单例**，否则并行跑的用例
    /// 会互相看见对方装的覆盖，红起来还没法复现。
    fn bundled() -> PromptRegistry {
        PromptRegistry::bundled()
    }

    /// **组装点唯一**的行为面守护：换一份提示词目录，四能力的 system 提示词
    /// 全部改口，且 `prompt_version` 随之改变。任何一条还从别处取字面量的能力，
    /// 都会在这里露馅（源码面的零残留由 `check-prompt-registry-single-source.py` 管）。
    #[test]
    fn every_capability_reads_its_system_prompt_from_the_registry() {
        use luna_core::{CapabilityPromptSet, REQUIRED_TEMPLATES};

        let registry = bundled();
        // 在随包基础上，把三条 system 模板换成可识别的哨兵（其余照抄，保证齐备）。
        let sets: Vec<CapabilityPromptSet> = REQUIRED_TEMPLATES
            .iter()
            .map(|(scope, keys)| CapabilityPromptSet {
                scope: *scope,
                locale: None,
                templates: keys
                    .iter()
                    .map(|(key, _)| {
                        let body = if *key == "system" {
                            format!("SENTINEL-{}", scope.as_str())
                        } else {
                            registry.template(*scope, key).expect("随包必须有这条模板")
                        };
                        ((*key).to_owned(), body)
                    })
                    .collect(),
            })
            .collect();
        registry
            .install_overlay(42, Some("terse".to_owned()), sets)
            .expect("齐备覆盖应被接受");

        let cases = [
            (
                JobKind::Translate {
                    text: TextRef::Inline("hi".into()),
                    source: None,
                    target: Lang::from("zh"),
                    instruction: None,
                    template: None,
                },
                "SENTINEL-translate",
            ),
            (
                JobKind::Summarize {
                    text: TextRef::Inline("hi".into()),
                    style: SummaryStyle::Bullets,
                    instruction: None,
                },
                "SENTINEL-summarize",
            ),
            (
                JobKind::Refine {
                    text: TextRef::Inline("hi".into()),
                    instruction: None,
                },
                "SENTINEL-refine",
            ),
        ];
        for (kind, sentinel) in cases {
            let assembled =
                build_chat_messages(&registry, &Job::new(kind, JobOrigin::FloatingMenu));
            assert_eq!(assembled.messages[0].role, "system");
            assert_eq!(
                assembled.messages[0].content, sentinel,
                "这条能力的 system 提示词没走注册表——它还在从代码里取字面量"
            );
            assert_eq!(assembled.prompt_version, 42, "上报版本必须是合成时那一版");
            assert_eq!(
                assembled.prompt_variant.as_deref(),
                Some("terse"),
                "变体与版本必须同一次取齐，账本按两个键归因"
            );
        }

        // 对话能力的 system 提示词来自角色预设（同一注册表的 chat 域），故在此一并钉住。
        let roles = luna_core::builtin_chat_presets(&registry);
        assert!(
            roles
                .iter()
                .all(|p| p.system_prompt.as_deref().is_some_and(|s| !s.is_empty())),
            "对话角色预设必须从注册表取到 system 提示词"
        );
    }

    #[test]
    fn translate_job_builds_system_plus_user() {
        let job = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("hello".into()),
                source: None,
                target: Lang::from("zh"),
                instruction: None,
                template: None,
            },
            JobOrigin::PopupTranslate,
        );
        let msgs = build_chat_messages(&bundled(), &job).messages;
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        // 语言全称（非 BCP-47 代码），且强制整段输出为目标语言。
        assert!(msgs[0].content.contains("Chinese"));
        assert!(msgs[0].content.contains("MUST be written in Chinese"));
        assert_eq!(msgs[1].role, "user");
        assert!(msgs[1].content.contains("hello"));
    }

    #[test]
    fn translate_job_appends_synthesized_instruction() {
        // A1：合成的风格指令落入 Translate.instruction，provider 渲染时附加。
        let job = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("hello".into()),
                source: None,
                target: Lang::from("zh"),
                instruction: Some(luna_core::Instruction::raw("Slack casual tone")),
                template: None,
            },
            JobOrigin::PopupTranslate,
        );
        let msgs = build_chat_messages(&bundled(), &job).messages;
        assert!(msgs[0].content.contains("Slack casual tone"));
        assert!(msgs[0].content.contains("Chinese"));
    }

    #[test]
    fn translate_job_declares_source_language_when_known() {
        let job = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("你好".into()),
                source: Some(Lang::from("zh")),
                target: Lang::from("en"),
                instruction: None,
                template: None,
            },
            JobOrigin::Recording,
        );
        let msgs = build_chat_messages(&bundled(), &job).messages;
        assert!(msgs[0].content.contains("into English"));
        assert!(msgs[0].content.contains("source text is in Chinese"));
    }

    #[test]
    fn translate_job_treats_questions_as_text_not_chat_turns() {
        let job = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("how are you".into()),
                source: Some(Lang::from("en")),
                target: Lang::from("zh"),
                instruction: None,
                template: None,
            },
            JobOrigin::PopupTranslate,
        );

        let msgs = build_chat_messages(&bundled(), &job).messages;
        assert!(
            msgs[0].content.contains("Do not answer questions"),
            "translate prompt must prevent small chat models from replying to short question-like input"
        );
        assert!(
            msgs[1].content.contains("Text to translate"),
            "translate input must be task-wrapped instead of sent as a raw chat turn"
        );
        assert!(
            msgs[1].content.contains("how are you"),
            "wrapped translate input must preserve the original text"
        );
        assert_ne!(msgs[1], super::user("how are you"));
    }

    #[test]
    fn summarize_job_appends_language_instruction() {
        let job = Job::new(
            JobKind::Summarize {
                text: TextRef::Inline("hello".into()),
                style: luna_core::SummaryStyle::Bullets,
                instruction: Some(luna_core::Instruction::raw("Write the summary in zh.")),
            },
            JobOrigin::FloatingMenu,
        );

        let msgs = build_chat_messages(&bundled(), &job).messages;
        assert!(msgs[0].content.contains("Write the summary in zh."));
        assert!(msgs[0].content.contains("Output only the summary"));
    }

    #[test]
    fn transcribe_job_yields_no_messages() {
        let job = Job::new(
            JobKind::Transcribe {
                audio: luna_core::AudioRef::Stream(luna_core::StreamId::new()),
                lang_hint: None,
                initial_prompt: None,
            },
            JobOrigin::Recording,
        );
        assert!(build_chat_messages(&bundled(), &job).messages.is_empty());
    }

    #[test]
    fn parse_delta_extracts_content() {
        let chunk = r#"{"choices":[{"delta":{"content":"你好"}}]}"#;
        assert_eq!(parse_delta(chunk), Some("你好".to_string()));
    }

    #[test]
    fn parse_delta_handles_role_only_and_empty() {
        assert_eq!(
            parse_delta(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#),
            None
        );
        assert_eq!(
            parse_delta(r#"{"choices":[{"delta":{"content":""}}]}"#),
            None
        );
        assert_eq!(parse_delta("not json"), None);
    }

    #[test]
    fn parse_delta_ignores_reasoning_tokens() {
        assert_eq!(
            parse_delta(r#"{"choices":[{"delta":{"reasoning":"hidden thoughts"}}]}"#),
            None
        );
    }

    struct NoEngine;

    impl LocalEngineSource for NoEngine {
        fn endpoint(&self) -> Option<crate::LocalEngineEndpoint> {
            None
        }
    }

    /// 就绪在一个固定地址上的「引擎」（打到本机假服务 / 无人监听的端口）。
    struct FixedEngine(&'static str);

    impl LocalEngineSource for FixedEngine {
        fn endpoint(&self) -> Option<crate::LocalEngineEndpoint> {
            Some(crate::LocalEngineEndpoint {
                base_url: self.0.to_owned(),
                api_key: "test-key".to_owned(),
            })
        }
    }

    #[test]
    fn local_engine_body_disables_thinking_and_asks_for_usage() {
        // Qwen3 / 3.5 模板默认先想后答：不关思考则正文为空、全进 `reasoning_content`。
        let config = LlmHttpConfig::local_engine(Arc::new(NoEngine), "chat");
        let body = build_request_body(&config, &[user("你好")], &JobOptions::default());
        assert_eq!(body["model"], "chat");
        assert_eq!(body["stream"], true);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(body["stream_options"]["include_usage"], true);
        // 未要求预算时不下发 max_tokens：其余任务沿用模型默认，不擅自改变既有行为。
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn output_budget_goes_to_the_local_engine() {
        let options = JobOptions {
            output_budget: Some(4_096),
            ..JobOptions::default()
        };
        let local = LlmHttpConfig::local_engine(Arc::new(NoEngine), "chat");
        let body = build_request_body(&local, &[user("长文")], &options);
        assert_eq!(body["max_tokens"], 4_096);
    }

    #[test]
    fn faithful_summarize_uses_the_instruction_verbatim_as_system_prompt() {
        // 整理稿（忠实改写）与「Summarize … concisely」正面相冲：外层前缀一旦套上，
        // 本地小模型多半听外层的，整理稿被静默做成另一份摘要。本测试锁住 Faithful 档
        // 的 system prompt **恰好等于** instruction。
        // 用例自造一句（不抄注册表里的整理稿模板——抄进用例等于允许它被抄回实现里，
        // 单一事实源门禁按整句指纹判红）。
        let instruction = "Turn this part of a transcript into clean Chinese prose. \
                           This is a rewrite, NOT a summary.";
        let job = Job::new(
            JobKind::Summarize {
                text: TextRef::Inline("原文".into()),
                style: SummaryStyle::Faithful,
                instruction: Some(luna_core::Instruction::raw(instruction)),
            },
            JobOrigin::Distill,
        );
        let messages = build_chat_messages(&bundled(), &job).messages;
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[0].content, instruction);
        assert!(!messages[0].content.contains("concisely"));
        assert!(!messages[0].content.contains("Output only the summary"));

        // 其余档维持前缀：悬浮菜单传的是"Write the summary in zh."这类半句片段，
        // 靠前缀撑起语义——一刀切替换会砸到它们。
        let brief = Job::new(
            JobKind::Summarize {
                text: TextRef::Inline("原文".into()),
                style: SummaryStyle::Bullets,
                instruction: Some(luna_core::Instruction::raw("Write the summary in zh.")),
            },
            JobOrigin::FloatingMenu,
        );
        let registry = bundled();
        let brief_messages = build_chat_messages(&registry, &brief).messages;
        // 期望值从注册表现取——把模板抄进用例里，等于允许它也被抄回实现里。
        let prefix = registry
            .template(luna_core::PromptScope::Summarize, "system")
            .expect("随包有摘要 system 模板");
        let prefix = prefix.split("{instruction}").next().unwrap().to_owned();
        assert!(brief_messages[0].content.starts_with(prefix.trim_end()));
        assert!(brief_messages[0]
            .content
            .contains("Write the summary in zh."));
    }

    /// OpenAI 兼容流自报的 `usage` 要取到；没有这个字段则如实为 `None`。
    ///
    /// **`None` 与 `Some((None, None))` 是两件事**：前者是"这一帧没有用量信息"（绝大多数
    /// 内容帧），后者是"有 usage 对象但里面没有能用的数字"。若把前者也当成后者，
    /// 每一个内容帧都会把已经收到的用量**覆盖成空**——那比一开始就不解析更糟。
    #[test]
    fn openai_usage_is_read_when_volunteered() {
        assert_eq!(
            parse_openai_usage(
                r#"{"choices":[],"usage":{"prompt_tokens":31,"completion_tokens":7,"total_tokens":38}}"#
            ),
            Some((Some(31), Some(7)))
        );
        // 普通内容帧没有 usage —— 必须是 None，不能是 Some((None, None))。
        assert_eq!(
            parse_openai_usage(r#"{"choices":[{"delta":{"content":"hi"}}]}"#),
            None
        );
        // 有 usage 对象但数字不可用（类型不符 / 溢出）：同样不产出，避免覆盖已收到的值。
        assert_eq!(
            parse_openai_usage(r#"{"usage":{"prompt_tokens":"31","completion_tokens":null}}"#),
            None
        );
        assert_eq!(parse_openai_usage("not json"), None);
    }

    /// 缺字段 / 类型不符 / 超出 `u32` 一律如实报「没有」，**绝不补一个编出来的数字**。
    ///
    /// 尤其是溢出：`as u32` 截断出来的用量看起来是真的，却在对账时对不上，
    /// 而且没有任何地方会因此报错——比直接留空糟得多。
    #[test]
    fn malformed_counts_report_absent_rather_than_guessing() {
        let cases = [
            r#"{"usage":{"prompt_tokens":"12","completion_tokens":null}}"#,
            r#"{"usage":{"prompt_tokens":-1,"completion_tokens":4294967296}}"#,
            r#"{"usage":{}}"#,
        ];
        for json in cases {
            assert_eq!(parse_openai_usage(json), None, "{json}");
        }
        // 一边可用、一边溢出：可用的那边照取，溢出的那边如实为 None。
        assert_eq!(
            parse_openai_usage(r#"{"usage":{"prompt_tokens":3,"completion_tokens":4294967296}}"#),
            Some((Some(3), None))
        );
    }

    #[test]
    fn missing_model_404_maps_to_model_missing_not_upstream() {
        // 裸字符串错误体。
        let err = classify_http_failure(
            &ProviderId::from(LOCAL_LLM_PROVIDER_ID),
            "qwen2.5:3b",
            404,
            r#"{"error":"model 'qwen2.5:3b' not found"}"#,
        );
        assert_eq!(err.code(), "MODEL_MISSING");
        assert!(!err.is_retriable());

        // OpenAI 兼容错误体（同为 404 not found）。
        let err = classify_http_failure(
            &ProviderId::from(LOCAL_LLM_PROVIDER_ID),
            "qwen2.5:3b",
            404,
            r#"{"error":{"message":"model 'qwen2.5:3b' not found","type":"not_found_error"}}"#,
        );
        assert_eq!(err.code(), "MODEL_MISSING");
    }

    #[test]
    fn rate_limit_is_distinct_and_other_failures_keep_the_parsed_message() {
        let err = classify_http_failure(
            &ProviderId::from(LOCAL_LLM_PROVIDER_ID),
            "m",
            429,
            r#"{"error":{"message":"rate limited"}}"#,
        );
        assert!(matches!(err, JobError::RateLimited { .. }));

        // 非 JSON 错误体回退原文（截断）。
        let err = classify_http_failure(&ProviderId::from(LOCAL_LLM_PROVIDER_ID), "m", 500, "boom");
        match err {
            JobError::Upstream { message, .. } => assert_eq!(message, "boom"),
            other => panic!("应为 Upstream，得到 {other:?}"),
        }
    }
}

#[cfg(test)]
mod rate_limit_tests {
    use super::*;

    fn provider() -> ProviderId {
        ProviderId::from(LOCAL_LLM_PROVIDER_ID)
    }

    /// 429 是可重试的限流，与其余上游错误分开。
    #[test]
    fn plain_429_is_a_distinct_retryable_rate_limit() {
        let err = classify_http_failure(&provider(), "m", 429, r#"{"error":"rate limited"}"#);
        assert!(
            matches!(err, JobError::RateLimited { .. }),
            "429 必须归 RateLimited：{err:?}"
        );
    }

    #[test]
    fn retry_after_accepts_seconds_and_http_date() {
        use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
        use std::time::{Duration, SystemTime};

        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("7"));
        assert_eq!(retry_after_secs_at(&headers, now), Some(7));

        let later = now + Duration::from_secs(11);
        headers.insert(
            RETRY_AFTER,
            HeaderValue::from_str(&httpdate::fmt_http_date(later)).unwrap(),
        );
        assert_eq!(retry_after_secs_at(&headers, now), Some(11));

        headers.insert(RETRY_AFTER, HeaderValue::from_static("soon"));
        assert_eq!(retry_after_secs_at(&headers, now), None);
    }
}
