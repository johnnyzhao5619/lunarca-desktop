//! ChatSession headless tests (TDD-01 §6.5).

use async_stream::stream;
use async_trait::async_trait;
use luna_core::{
    AIProvider, CancelToken, Capability, CapabilitySet, ChatMessage, ChatRole, Job, JobEvent,
    JobEventStream, JobKind, JobResult, ModelId, PartialPayload, ProviderDescriptor,
    ProviderHealth, ProviderId, ProviderLocation, ResultPayload, Timestamp, Timings, Usage,
};
use luna_jobqueue::{AdmissionCapacity, ChatSession, ChatSessionConfig, JobQueue};
use luna_provider::{ProviderRegistry, TokenBudget, TokenCount, TokenCountMethod, TokenCounter};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct CharTokenCounter;

impl TokenCounter for CharTokenCounter {
    fn count_chat_tokens(&self, messages: &[ChatMessage]) -> TokenCount {
        let tokens = messages
            .iter()
            .map(|m| 2 + m.content.chars().count() as u32)
            .sum();
        TokenCount {
            tokens,
            method: TokenCountMethod::Exact,
            includes_chat_template: true,
        }
    }
}

struct RecordingChatProvider {
    id: ProviderId,
    seen: Arc<Mutex<Vec<Vec<ChatMessage>>>>,
    seen_local_only: Arc<Mutex<Vec<bool>>>,
    script: ChatScript,
}

#[derive(Clone)]
enum ChatScript {
    Text(&'static str),
    HangAfterTokens(Vec<&'static str>),
}

impl RecordingChatProvider {
    fn text(seen: Arc<Mutex<Vec<Vec<ChatMessage>>>>, text: &'static str) -> Arc<Self> {
        Arc::new(Self {
            id: ProviderId::from("chat-provider"),
            seen,
            seen_local_only: Arc::new(Mutex::new(Vec::new())),
            script: ChatScript::Text(text),
        })
    }

    fn hanging(seen: Arc<Mutex<Vec<Vec<ChatMessage>>>>, tokens: Vec<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            id: ProviderId::from("chat-provider"),
            seen,
            seen_local_only: Arc::new(Mutex::new(Vec::new())),
            script: ChatScript::HangAfterTokens(tokens),
        })
    }

    fn text_with_routing(
        seen: Arc<Mutex<Vec<Vec<ChatMessage>>>>,
        seen_local_only: Arc<Mutex<Vec<bool>>>,
        text: &'static str,
    ) -> Arc<Self> {
        Arc::new(Self {
            id: ProviderId::from("chat-provider"),
            seen,
            seen_local_only,
            script: ChatScript::Text(text),
        })
    }
}

#[async_trait]
impl AIProvider for RecordingChatProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id.clone(),
            display_name: "chat-provider".into(),
            location: ProviderLocation::LocalService,
            model: None,
            license_summary: None,
        }
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet {
            capabilities: [Capability::Chat].into_iter().collect(),
            streaming: true,
            location: ProviderLocation::LocalService,
            explicit_only: false,
        }
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::ready()
    }

    async fn execute(&self, job: Job, cancel: CancelToken) -> JobEventStream {
        if let JobKind::Chat { messages, .. } = &job.kind {
            self.seen.lock().unwrap().push(messages.clone());
        }
        self.seen_local_only
            .lock()
            .unwrap()
            .push(job.routing.local_only);
        let provider_id = self.id.clone();
        let script = self.script.clone();
        let job_id = job.id;
        Box::pin(stream! {
            yield JobEvent::Started;
            match script {
                ChatScript::Text(text) => {
                    for ch in text.chars() {
                        yield JobEvent::Partial(PartialPayload::Token(ch.to_string()));
                    }
                    yield JobEvent::Finished(JobResult {
                        job_id,
                        provider_id,
                        model_id: Some(ModelId::from("test-chat-model")),
                        payload: ResultPayload::Text(text.to_string()),
                        usage: Usage::default(),
                        timings: Timings::default(),
                        output_truncated: None,
                        finished_at: Timestamp::ZERO,
                    });
                }
                ChatScript::HangAfterTokens(tokens) => {
                    for token in tokens {
                        yield JobEvent::Partial(PartialPayload::Token(token.to_string()));
                    }
                    loop {
                        if cancel.is_cancelled() {
                            yield JobEvent::Cancelled;
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                }
            }
        })
    }

    fn estimate(&self, _job: &Job) -> Option<luna_core::UsageEstimate> {
        None
    }
}

fn queue_with(provider: Arc<dyn AIProvider>) -> JobQueue {
    let registry = ProviderRegistry::new();
    registry.register(provider);
    JobQueue::new(
        Arc::new(registry),
        AdmissionCapacity {
            compute: 1,
            session: 1,
        },
    )
}

fn session_with_budget(context_window: u32, reserved_output_tokens: u32) -> ChatSession {
    ChatSession::new(
        ChatSessionConfig {
            system_prompt: Some("system rules".into()),
            token_budget: TokenBudget {
                context_window,
                reserved_output_tokens,
            },
            ..ChatSessionConfig::default()
        },
        Arc::new(CharTokenCounter),
    )
}

#[tokio::test]
async fn chat_session_truncates_context_within_n_ctx_and_preserves_system() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let queue = queue_with(RecordingChatProvider::text(seen.clone(), "ok"));
    let mut session = session_with_budget(80, 10);

    for i in 0..6 {
        session.push_history(ChatMessage::new(
            ChatRole::User,
            format!("old user message {i}"),
        ));
        session.push_history(ChatMessage::new(
            ChatRole::Assistant,
            format!("old assistant message {i}"),
        ));
    }

    let turn = session
        .send("new prompt with enough length", None, &queue)
        .expect("send should start");
    let outcome = turn.drain_into(&mut session).await;
    assert!(outcome.truncated);

    let contexts = seen.lock().unwrap();
    let submitted = contexts.last().expect("provider saw chat job");
    let count = CharTokenCounter.count_chat_tokens(submitted).tokens;
    assert!(
        count <= 70,
        "submitted context must leave reserved output tokens inside n_ctx"
    );
    assert_eq!(submitted.first().unwrap().role, ChatRole::System);
    assert_eq!(submitted.first().unwrap().content, "system rules");
    assert_eq!(submitted.last().unwrap().role, ChatRole::User);
    assert_eq!(
        submitted.last().unwrap().content,
        "new prompt with enough length"
    );
}

#[tokio::test]
async fn regenerate_reuses_context_and_replaces_previous_assistant() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let queue = queue_with(RecordingChatProvider::text(seen.clone(), "answer"));
    let mut session = session_with_budget(128, 16);

    let turn = session
        .send("hello", None, &queue)
        .expect("send should start");
    turn.drain_into(&mut session).await;
    assert_eq!(
        session.messages().last(),
        Some(&ChatMessage::new(ChatRole::Assistant, "answer"))
    );

    let regenerate = session
        .regenerate(None, &queue)
        .expect("regenerate should start");
    regenerate.drain_into(&mut session).await;

    let contexts = seen.lock().unwrap();
    assert_eq!(contexts.len(), 2);
    assert_eq!(contexts[0], contexts[1]);
    assert_eq!(
        session
            .messages()
            .iter()
            .filter(|m| m.role == ChatRole::Assistant)
            .count(),
        1,
        "default regenerate replaces the previous assistant reply"
    );
}

#[tokio::test]
async fn stopping_generation_keeps_tokens_already_received() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let queue = queue_with(RecordingChatProvider::hanging(seen, vec!["par", "tial"]));
    let mut session = session_with_budget(128, 16);

    let turn = session.send("stop after partial", None, &queue).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    session.stop(&turn);
    let outcome = turn.drain_into(&mut session).await;

    assert!(outcome.cancelled);
    assert_eq!(
        session.messages().last(),
        Some(&ChatMessage::new(ChatRole::Assistant, "partial"))
    );
}

#[tokio::test]
async fn drain_into_with_forwards_each_streaming_token() {
    // PRD-03-002 [P0] 流式输出：每个 token 应即时经回调透传（供 Tauri 推给前端），
    // 且回调收到的拼接结果与最终 assistant 文本一致。
    let seen = Arc::new(Mutex::new(Vec::new()));
    let queue = queue_with(RecordingChatProvider::text(seen, "你好"));
    let mut session = session_with_budget(128, 16);

    let streamed = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = streamed.clone();
    let turn = session.send("hi", None, &queue).unwrap();
    let outcome = turn
        .drain_into_with(&mut session, move |token| {
            sink.lock().unwrap().push(token.to_string());
        })
        .await;

    let tokens = streamed.lock().unwrap();
    assert_eq!(
        *tokens,
        vec!["你".to_string(), "好".to_string()],
        "应逐 token 透传"
    );
    assert_eq!(tokens.concat(), outcome.assistant);
    assert!(outcome.finished);
}

#[tokio::test]
async fn local_only_routing_is_applied_to_submitted_chat_job() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_local_only = Arc::new(Mutex::new(Vec::new()));
    let queue = queue_with(RecordingChatProvider::text_with_routing(
        seen,
        seen_local_only.clone(),
        "ok",
    ));
    let mut session = session_with_budget(128, 16);

    session.set_local_only(true);
    let turn = session.send("hi", None, &queue).unwrap();
    turn.drain_into(&mut session).await;

    assert_eq!(*seen_local_only.lock().unwrap(), vec![true]);
}

#[test]
fn new_chat_session_has_no_previous_process_history() {
    let mut first = session_with_budget(128, 16);
    first.push_history(ChatMessage::new(ChatRole::User, "private prompt"));
    first.push_history(ChatMessage::new(ChatRole::Assistant, "private answer"));

    let second = session_with_budget(128, 16);

    assert_eq!(second.messages().len(), 1);
    assert_eq!(second.messages()[0].role, ChatRole::System);
    assert_eq!(second.messages()[0].content, "system rules");
}

/// 瞬态上下文（PRD-16 D3 / ADR-026 ⑤）：随本轮提交，**绝不进会话历史**。
///
/// 这条守护挡的是一个静默故障：若检索摘录被落进 `messages`，归档笔记会每轮携带数 KB
/// 摘录，历史载入后重复计费且不可读——而界面上一切正常，没有任何地方会报错。
#[tokio::test]
async fn grounding_context_reaches_provider_but_never_enters_history() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let queue = queue_with(RecordingChatProvider::text(seen.clone(), "answer"));
    let mut session = session_with_budget(4096, 256);

    let turn = session
        .send(
            "项目 X 的进展如何",
            Some("[1] 项目周报\n> 本周完成部署"),
            &queue,
        )
        .expect("send should start");
    turn.drain_into(&mut session).await;

    let contexts = seen.lock().unwrap();
    let submitted = contexts.last().expect("provider saw chat job");
    assert!(
        submitted.iter().any(|m| m.content.contains("本周完成部署")),
        "摘录必须到达 Provider：{submitted:?}"
    );
    assert_eq!(
        submitted.first().unwrap().content,
        "system rules",
        "预设 system 仍在队首，瞬态块排其后"
    );

    // 会话历史里只有用户那句话与回答，没有任何摘录。
    for message in session.messages() {
        assert!(
            !message.content.contains("本周完成部署"),
            "摘录漏进会话历史：{message:?}"
        );
    }
    assert_eq!(session.messages().len(), 3, "system + user + assistant");
}

/// 瞬态块必须**为自己留出预算**：历史刚好填满时不能把摘录静默挤掉（检索白做）。
#[tokio::test]
async fn grounding_context_reserves_budget_against_long_history() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let queue = queue_with(RecordingChatProvider::text(seen.clone(), "ok"));
    let mut session = session_with_budget(200, 20);

    for i in 0..12 {
        session.push_history(ChatMessage::new(ChatRole::User, format!("历史提问 {i}")));
        session.push_history(ChatMessage::new(
            ChatRole::Assistant,
            format!("历史回答 {i}"),
        ));
    }

    let turn = session
        .send("新的问题", Some("检索到的关键依据在此"), &queue)
        .expect("send should start");
    turn.drain_into(&mut session).await;

    let contexts = seen.lock().unwrap();
    let submitted = contexts.last().expect("provider saw chat job");
    assert!(
        submitted
            .iter()
            .any(|m| m.content.contains("检索到的关键依据在此")),
        "历史再长也不能挤掉本轮依据：{submitted:?}"
    );
    assert!(
        submitted.last().unwrap().content.contains("新的问题"),
        "用户自己的问题永远最后被裁"
    );
    let count = CharTokenCounter.count_chat_tokens(submitted).tokens;
    assert!(count <= 180, "仍须守住 n_ctx 预算：{count}");
}

/// 重新生成时瞬态上下文**由调用方重新给**——「剔除某篇再答一次」（PRD-16-006）
/// 正是靠这条成立；若缓存住上一轮的摘录，那个动作会静默失效。
#[tokio::test]
async fn regenerate_takes_fresh_grounding_not_the_previous_one() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let queue = queue_with(RecordingChatProvider::text(seen.clone(), "answer"));
    let mut session = session_with_budget(4096, 256);

    session
        .send("问题", Some("旧的检索摘录"), &queue)
        .expect("send")
        .drain_into(&mut session)
        .await;
    session
        .regenerate(Some("新的检索摘录"), &queue)
        .expect("regenerate")
        .drain_into(&mut session)
        .await;

    let contexts = seen.lock().unwrap();
    let second = contexts.last().unwrap();
    assert!(second.iter().any(|m| m.content.contains("新的检索摘录")));
    assert!(
        !second.iter().any(|m| m.content.contains("旧的检索摘录")),
        "上一轮的摘录绝不能残留：{second:?}"
    );
}
