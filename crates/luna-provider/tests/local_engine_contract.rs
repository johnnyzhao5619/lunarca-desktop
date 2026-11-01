//! 随包推理引擎 Provider 的契约（ADR-143，P3a）：请求打到哪、带什么、每种响应落到哪个终态。
//!
//! 用真实 loopback TCP 而不是假 HTTP 客户端：要钉住的是**线上的字节**——钥匙走 Bearer、
//! 关思考的模板参数真的发出去了、地址是请求那一刻现问的。响应体取自 P3 预探时
//! `llama-server` b11429 路由模式的真实回包（形状见 `local_engine` 模块文档）。

use std::sync::{Arc, Mutex};

use luna_core::{
    AIProvider, CancelToken, Job, JobError, JobEvent, JobKind, JobOrigin, Lang, ResultPayload,
    TextRef, TruncatedBy,
};
use luna_provider::{
    check_invariants, EmbedHttpConfig, EmbedHttpProvider, LlmHttpConfig, LlmHttpProvider,
    LocalEngineEndpoint, LocalEngineSource, TerminalKind, LOCAL_LLM_PROVIDER_ID,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// 可在测试中途改地址的引擎事实源（模拟引擎重启换端口）。
#[derive(Default)]
struct Source(Mutex<Option<LocalEngineEndpoint>>);

impl Source {
    fn at(port: u16) -> Arc<Self> {
        let source = Arc::new(Self::default());
        source.point_to(port);
        source
    }
    fn point_to(&self, port: u16) {
        *self.0.lock().unwrap() = Some(LocalEngineEndpoint {
            base_url: format!("http://127.0.0.1:{port}"),
            api_key: format!("key-{port}"),
        });
    }
}

impl LocalEngineSource for Source {
    fn endpoint(&self) -> Option<LocalEngineEndpoint> {
        self.0.lock().unwrap().clone()
    }
}

/// 收到的一次请求。
#[derive(Debug, Default, Clone)]
struct Seen {
    head: String,
    body: serde_json::Value,
}

/// 起一个只答一次的 HTTP 端点；返回端口与「它收到了什么」。
async fn serve_once(
    status: u16,
    content_type: &str,
    body: String,
) -> (u16, Arc<Mutex<Option<Seen>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(None));
    let probe = Arc::clone(&seen);
    let content_type = content_type.to_owned();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut raw = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        // 读到头 + Content-Length 那么多字节为止。
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            raw.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&raw).into_owned();
            if let Some(split) = text.find("\r\n\r\n") {
                let length = text[..split]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if raw.len() >= split + 4 + length {
                    *probe.lock().unwrap() = Some(Seen {
                        head: text[..split].to_owned(),
                        body: serde_json::from_slice(&raw[split + 4..split + 4 + length])
                            .unwrap_or_default(),
                    });
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        let reason = if status == 200 { "OK" } else { "Error" };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.shutdown().await.ok();
    });
    (port, seen)
}

fn translate_job(output_budget: Option<u32>) -> Job {
    let mut job = Job::new(
        JobKind::Translate {
            text: TextRef::Inline("hello".into()),
            source: None,
            target: Lang::from("zh"),
            instruction: None,
            template: None,
        },
        JobOrigin::PopupTranslate,
    );
    job.options.output_budget = output_budget;
    job
}

fn embed_job(texts: &[&str]) -> Job {
    Job::new(
        JobKind::Embed {
            texts: texts.iter().map(|t| (*t).to_owned()).collect(),
            purpose: luna_core::EmbeddingPurpose::Document,
        },
        JobOrigin::Recording,
    )
}

/// 跑完一条流，返回终态事件（并顺带过一遍通用流式不变量）。
async fn terminal_of(provider: &dyn AIProvider, job: Job) -> JobEvent {
    let events: Vec<JobEvent> = {
        use futures::StreamExt as _;
        provider
            .execute(job, CancelToken::new())
            .await
            .collect()
            .await
    };
    events.last().cloned().expect("流必须有终态")
}

fn sse(frames: &[&str]) -> String {
    frames
        .iter()
        .map(|f| format!("data: {f}\n\n"))
        .collect::<String>()
        + "data: [DONE]\n\n"
}

#[tokio::test]
async fn chat_goes_to_the_engine_with_its_key_and_thinking_off() {
    let body = sse(&[
        r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":"你"}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"content":"好"}}]}"#,
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"length"}]}"#,
        r#"{"choices":[],"usage":{"completion_tokens":2,"prompt_tokens":16,"total_tokens":18}}"#,
    ]);
    let (port, seen) = serve_once(200, "text/event-stream", body).await;
    let provider = LlmHttpProvider::new(LlmHttpConfig::local_engine(
        Source::at(port),
        "qwen3.5-2b-q8_0-gguf",
    ));

    let JobEvent::Finished(result) = terminal_of(&provider, translate_job(Some(64))).await else {
        panic!("应正常收尾");
    };
    assert!(matches!(result.payload, ResultPayload::Text(ref t) if t == "你好"));
    assert_eq!(result.usage.tokens_in, Some(16));
    assert_eq!(result.usage.tokens_out, Some(2));
    // 给了输出预算而 `finish_reason = length`：触顶的是调用方自己的额度。
    assert_eq!(result.output_truncated, Some(TruncatedBy::Caller));

    let seen = seen.lock().unwrap().clone().unwrap();
    assert!(
        seen.head.starts_with("POST /v1/chat/completions "),
        "{}",
        seen.head
    );
    assert!(seen
        .head
        .to_ascii_lowercase()
        .contains(&format!("authorization: bearer key-{port}")));
    assert_eq!(seen.body["model"], "qwen3.5-2b-q8_0-gguf");
    assert_eq!(seen.body["stream"], true);
    assert_eq!(seen.body["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(seen.body["stream_options"]["include_usage"], true);
    assert_eq!(seen.body["max_tokens"], 64);
    // 上下文窗不逐请求下发：它在引擎加载时定死。
    assert!(seen.body.get("options").is_none());
}

#[tokio::test]
async fn hitting_the_loaded_window_without_a_budget_is_attributed_to_the_engine() {
    let body = sse(&[
        r#"{"choices":[{"index":0,"delta":{"content":"半"}}]}"#,
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"length"}]}"#,
    ]);
    let (port, _) = serve_once(200, "text/event-stream", body).await;
    let provider = LlmHttpProvider::new(LlmHttpConfig::local_engine(Source::at(port), "m"));
    let JobEvent::Finished(result) = terminal_of(&provider, translate_job(None)).await else {
        panic!("应正常收尾");
    };
    assert_eq!(result.output_truncated, Some(TruncatedBy::Upstream));
}

#[tokio::test]
async fn a_normal_stop_reports_no_truncation() {
    let body = sse(&[
        r#"{"choices":[{"index":0,"delta":{"content":"完"}}]}"#,
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
    ]);
    let (port, _) = serve_once(200, "text/event-stream", body).await;
    let provider = LlmHttpProvider::new(LlmHttpConfig::local_engine(Source::at(port), "m"));
    let JobEvent::Finished(result) = terminal_of(&provider, translate_job(Some(64))).await else {
        panic!("应正常收尾");
    };
    assert_eq!(result.output_truncated, None);
}

/// 引擎没就绪：不拨号、立即 `ProviderUnavailable`，流仍守通用不变量。
#[tokio::test]
async fn an_engine_that_is_not_ready_is_unavailable_without_dialing() {
    let provider = LlmHttpProvider::new(LlmHttpConfig::local_engine(
        Arc::new(Source::default()),
        "m",
    ));
    let summary = check_invariants(
        provider
            .execute(translate_job(None), CancelToken::new())
            .await,
    )
    .await;
    assert_eq!(summary.terminal, Some(TerminalKind::Failed));
    assert!(matches!(
        terminal_of(&provider, translate_job(None)).await,
        JobEvent::Failed(JobError::ProviderUnavailable { provider }) if provider.as_str() == LOCAL_LLM_PROVIDER_ID
    ));

    let embed = EmbedHttpProvider::new(EmbedHttpConfig::local_engine(
        Arc::new(Source::default()),
        "m",
    ));
    assert!(matches!(
        terminal_of(&embed, embed_job(&["a"])).await,
        JobEvent::Failed(JobError::ProviderUnavailable { .. })
    ));
}

/// 引擎重启换了端口与钥匙：同一个 Provider 实例的下一次请求就打到新地址。
#[tokio::test]
async fn the_address_is_asked_for_at_request_time() {
    let ok =
        || sse(&[r#"{"choices":[{"index":0,"delta":{"content":"x"},"finish_reason":"stop"}]}"#]);
    let (first, _) = serve_once(200, "text/event-stream", ok()).await;
    let source = Source::at(first);
    let provider = LlmHttpProvider::new(LlmHttpConfig::local_engine(source.clone(), "m"));
    assert!(matches!(
        terminal_of(&provider, translate_job(None)).await,
        JobEvent::Finished(_)
    ));

    let (second, seen) = serve_once(200, "text/event-stream", ok()).await;
    source.point_to(second);
    assert!(matches!(
        terminal_of(&provider, translate_job(None)).await,
        JobEvent::Finished(_)
    ));
    let head = seen
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .head
        .to_ascii_lowercase();
    assert!(
        head.contains(&format!("bearer key-{second}")),
        "新钥匙必须随新地址一起换"
    );
}

#[tokio::test]
async fn an_overlong_prompt_is_input_too_long_not_a_retriable_upstream_error() {
    let (port, _) = serve_once(
        400,
        "application/json",
        r#"{"error":{"code":400,"message":"request (2262 tokens) exceeds the available context size (2048 tokens), try increasing it","type":"exceed_context_size_error","n_prompt_tokens":2262,"n_ctx":2048}}"#.into(),
    )
    .await;
    let provider = LlmHttpProvider::new(LlmHttpConfig::local_engine(Source::at(port), "m"));
    let JobEvent::Failed(error) = terminal_of(&provider, translate_job(None)).await else {
        panic!("应失败");
    };
    assert!(matches!(
        error,
        JobError::InputTooLong {
            input_tokens: Some(2262),
            window: Some(2048),
            ..
        }
    ));
    assert!(!error.is_retriable());
    assert_eq!(error.code(), "INPUT_TOO_LONG");
}

#[tokio::test]
async fn an_unknown_model_is_model_missing_even_though_the_engine_says_400() {
    let (port, _) = serve_once(
        400,
        "application/json",
        r#"{"error":{"code":400,"message":"model 'm' not found","type":"invalid_request_error"}}"#
            .into(),
    )
    .await;
    let provider = LlmHttpProvider::new(LlmHttpConfig::local_engine(Source::at(port), "m"));
    assert!(matches!(
        terminal_of(&provider, translate_job(None)).await,
        JobEvent::Failed(JobError::ModelMissing { .. })
    ));
}

#[tokio::test]
async fn an_error_frame_mid_stream_fails_the_job() {
    let body = sse(&[
        r#"{"choices":[{"index":0,"delta":{"content":"半"}}]}"#,
        r#"{"error":{"code":500,"message":"slot crashed","type":"server_error"}}"#,
    ]);
    let (port, _) = serve_once(200, "text/event-stream", body).await;
    let provider = LlmHttpProvider::new(LlmHttpConfig::local_engine(Source::at(port), "m"));
    let summary = check_invariants(
        provider
            .execute(translate_job(None), CancelToken::new())
            .await,
    )
    .await;
    assert_eq!(summary.terminal, Some(TerminalKind::Failed));
    assert!(!summary.saw_event_after_terminal);
}

#[tokio::test]
async fn embeddings_go_to_the_engine_and_come_back_in_input_order() {
    // 引擎不承诺 `data` 按输入顺序排列；按 `index` 归位，否则向量会指向别的分片。
    let (port, seen) = serve_once(
        200,
        "application/json",
        r#"{"data":[{"index":1,"embedding":[0.0,1.0],"object":"embedding"},{"index":0,"embedding":[1.0,0.0],"object":"embedding"}],"usage":{"prompt_tokens":4,"total_tokens":4}}"#.into(),
    )
    .await;
    let provider = EmbedHttpProvider::new(EmbedHttpConfig::local_engine(
        Source::at(port),
        "bge-m3-q8_0-gguf",
    ));
    let JobEvent::Finished(result) = terminal_of(&provider, embed_job(&["甲", "乙"])).await
    else {
        panic!("应成功");
    };
    let ResultPayload::Embeddings(vectors) = result.payload else {
        panic!("应为向量");
    };
    assert_eq!(vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);

    let seen = seen.lock().unwrap().clone().unwrap();
    assert!(
        seen.head.starts_with("POST /v1/embeddings "),
        "{}",
        seen.head
    );
    assert!(seen
        .head
        .to_ascii_lowercase()
        .contains(&format!("bearer key-{port}")));
    assert_eq!(seen.body["model"], "bge-m3-q8_0-gguf");
    assert_eq!(seen.body["input"], serde_json::json!(["甲", "乙"]));
    // 引擎超窗就拒，从不截断。
    assert!(seen.body.get("truncate").is_none());
}

#[tokio::test]
async fn an_oversized_embedding_batch_is_input_too_long_despite_the_500() {
    let (port, _) = serve_once(
        500,
        "application/json",
        r#"{"error":{"code":500,"message":"input (3603 tokens) is too large to process. increase the physical batch size (current batch size: 2048)","type":"server_error"}}"#.into(),
    )
    .await;
    let provider = EmbedHttpProvider::new(EmbedHttpConfig::local_engine(Source::at(port), "m"));
    assert!(matches!(
        terminal_of(&provider, embed_job(&["长"])).await,
        JobEvent::Failed(JobError::InputTooLong {
            input_tokens: Some(3603),
            window: Some(2048),
            ..
        })
    ));
}
