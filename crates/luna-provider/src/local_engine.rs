//! 随包推理引擎（`llama-server` 路由器，ADR-143）的
//! Provider 侧接缝：**当前地址从哪来**，以及它那几种**自有的错误形状**怎么归一。
//!
//! # 地址为什么是「每次请求现问」
//!
//! 引擎以 `--port 0` 起、每次重启换端口，钥匙也是每次进程启动现生成的（只走环境变量
//! `LLAMA_API_KEY`，P0 实测 CLI / preset 里的钥匙传不到子实例）。把地址在构造 Provider
//! 时钉死，就得在每次引擎重启后重装一遍 Provider——那会让 `luna-engine` 的监管层反过来
//! 依赖 Provider 注册表。改为请求时向 [`LocalEngineSource`] 现问：引擎没就绪就如实
//! `ProviderUnavailable`，就绪了就用当下的端口与钥匙，两边互不知道对方的存在。
//!
//! # 错误形状（2026-10-07 P3 预探，b11429 路由模式实测）
//!
//! | 情形 | 状态码 | 体 |
//! | --- | --- | --- |
//! | 对话超窗 | 400 | `{"error":{"type":"exceed_context_size_error","n_prompt_tokens":N,"n_ctx":M,…}}` |
//! | 编码器嵌入超批 | **500** | `{"error":{"type":"server_error","message":"input (N tokens) is too large to process. …(current batch size: M)"}}` |
//! | 未知模型 | **400** | `{"error":{"type":"invalid_request_error","message":"model 'x' not found"}}` |
//! | 钥匙不对 | 401 | `{"error":{"type":"authentication_error",…}}` |
//!
//! 后两种与 OpenAI 惯例不同（未知模型不是 404、超批不是 4xx），只按状态码
//! 分类会把「模型没装」报成可重试的上游错误、把「输入太长」报成可重试的 5xx。

use luna_core::{JobError, ModelId, ProviderId};
use serde_json::Value;

/// 随包引擎默认 / 质量档 LLM Provider id。
pub const LOCAL_LLM_PROVIDER_ID: &str = "local-llm";
/// 随包引擎实时档 LLM Provider id（只接显式定向）。
pub const LOCAL_LLM_REALTIME_PROVIDER_ID: &str = "local-llm-realtime";
/// 随包引擎嵌入 Provider id。
pub const LOCAL_EMBED_PROVIDER_ID: &str = "local-embed";

/// 引擎此刻的地址与钥匙。
#[derive(Clone, PartialEq, Eq)]
pub struct LocalEngineEndpoint {
    /// 路由器根地址（**不含** `/v1`），如 `http://127.0.0.1:53817`。
    pub base_url: String,
    /// 本次进程的钥匙（Bearer）。
    pub api_key: String,
}

impl std::fmt::Debug for LocalEngineEndpoint {
    // 钥匙不进日志：`Debug` 会被错误链与 `eprintln!` 带出去。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalEngineEndpoint")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

/// 「引擎现在在哪」的事实源（应用层用监管器实现）。
pub trait LocalEngineSource: Send + Sync {
    /// 就绪时的地址；没起、起着、熔断时为 `None`。
    fn endpoint(&self) -> Option<LocalEngineEndpoint>;
}

/// 把引擎特有的失败归一成可行动的错误；不是这几种形状时返回 `None`，交回通用分类。
#[must_use]
pub(crate) fn classify_engine_failure(
    provider: &ProviderId,
    model: &str,
    status: u16,
    body: &str,
) -> Option<JobError> {
    let error = serde_json::from_str::<Value>(body)
        .ok()?
        .get("error")?
        .clone();
    let kind = error
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if kind == "exceed_context_size_error" {
        return Some(JobError::InputTooLong {
            provider: provider.clone(),
            input_tokens: count(&error, "n_prompt_tokens"),
            window: count(&error, "n_ctx"),
        });
    }
    // 编码器（bge-m3）的物理批先于上下文触发，回的是 500 + 一句话；数字只在句子里。
    if message.contains("is too large to process") {
        return Some(JobError::InputTooLong {
            provider: provider.clone(),
            input_tokens: number_after(message, "input ("),
            window: number_after(message, "current batch size: "),
        });
    }
    if status == 400 && message.starts_with("model '") && message.ends_with("' not found") {
        return Some(JobError::ModelMissing {
            model: ModelId::from(model),
            hint: format!("模型 {model} 未装进本机推理引擎：请在 设置 → 本地模型 中安装"),
        });
    }
    None
}

fn count(value: &Value, key: &str) -> Option<u32> {
    u32::try_from(value.get(key)?.as_u64()?).ok()
}

/// 句子里紧跟在 `marker` 之后的那个整数。
fn number_after(message: &str, marker: &str) -> Option<u32> {
    let rest = &message[message.find(marker)? + marker.len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(status: u16, body: &str) -> Option<JobError> {
        classify_engine_failure(
            &ProviderId::from(LOCAL_LLM_PROVIDER_ID),
            "chat",
            status,
            body,
        )
    }

    /// 三种形状都取自 P3 预探的真实响应体（b11429 路由模式）。
    #[test]
    fn engine_failures_become_actionable_errors() {
        let chat = r#"{"error":{"code":400,"message":"request (2262 tokens) exceeds the available context size (2048 tokens), try increasing it","type":"exceed_context_size_error","n_prompt_tokens":2262,"n_ctx":2048}}"#;
        assert!(matches!(
            classify(400, chat),
            Some(JobError::InputTooLong {
                input_tokens: Some(2262),
                window: Some(2048),
                ..
            })
        ));

        let embed = r#"{"error":{"code":500,"message":"input (3603 tokens) is too large to process. increase the physical batch size (current batch size: 2048)","type":"server_error"}}"#;
        assert!(matches!(
            classify(500, embed),
            Some(JobError::InputTooLong {
                input_tokens: Some(3603),
                window: Some(2048),
                ..
            })
        ));

        let missing = r#"{"error":{"code":400,"message":"model 'nope' not found","type":"invalid_request_error"}}"#;
        assert!(matches!(
            classify(400, missing),
            Some(JobError::ModelMissing { .. })
        ));
    }

    #[test]
    fn other_failures_fall_back_to_generic_classification() {
        assert!(classify(
            401,
            r#"{"error":{"message":"Invalid API Key","type":"authentication_error","code":401}}"#
        )
        .is_none());
        assert!(classify(500, r#"{"error":{"message":"boom","type":"server_error"}}"#).is_none());
        assert!(classify(500, "proxy error: Failed to read connection").is_none());
    }

    #[test]
    fn the_key_never_reaches_debug_output() {
        let endpoint = LocalEngineEndpoint {
            base_url: "http://127.0.0.1:1".into(),
            api_key: "secret-key-value".into(),
        };
        assert!(!format!("{endpoint:?}").contains("secret-key-value"));
    }
}
