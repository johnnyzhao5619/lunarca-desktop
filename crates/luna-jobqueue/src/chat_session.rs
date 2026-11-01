//! Rust 内存态 ChatSession（TDD-01 §6.5）。

use crate::{JobHandle, JobSink};
use luna_core::{
    ChatMessage, ChatRole, Job, JobError, JobEvent, JobKind, JobOrigin, Priority, ResultPayload,
    RoutingHint,
};
use luna_provider::{TokenBudget, TokenCounter};
use std::sync::Arc;

/// ChatSession 配置。
#[derive(Clone, Default)]
pub struct ChatSessionConfig {
    /// 可选 system prompt（角色预设的渲染结果）。
    pub system_prompt: Option<String>,
    /// 路由偏好。
    pub routing: RoutingHint,
    /// 上下文预算。
    pub token_budget: TokenBudget,
}

/// ChatSession 错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatSessionError {
    /// 用户消息为空。
    EmptyUserMessage,
    /// 无可重新生成的用户轮次。
    NoUserTurn,
}

/// 一次提交前组装好的上下文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatContext {
    /// 实际提交给 `JobKind::Chat` 的 messages。
    pub messages: Vec<ChatMessage>,
    /// 是否发生截断。
    pub truncated: bool,
    /// 提交上下文的 token 数。
    pub tokens_in: u32,
    /// 输入预算。
    pub input_limit: u32,
}

/// ChatSession：唯一事实源为内存 messages，退出不落盘。
pub struct ChatSession {
    messages: Vec<ChatMessage>,
    routing: RoutingHint,
    token_budget: TokenBudget,
    counter: Arc<dyn TokenCounter>,
}

impl ChatSession {
    /// 构造内存会话。
    #[must_use]
    pub fn new(config: ChatSessionConfig, counter: Arc<dyn TokenCounter>) -> Self {
        let mut messages = Vec::new();
        if let Some(system) = config.system_prompt {
            if !system.trim().is_empty() {
                messages.push(ChatMessage::new(ChatRole::System, system));
            }
        }
        Self {
            messages,
            routing: config.routing,
            token_budget: config.token_budget,
            counter,
        }
    }

    /// 当前内存历史。
    #[must_use]
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// 更新严格本地路由开关。命令层在每次提交前从设置快照刷新，避免会话长期持有过期策略。
    pub fn set_local_only(&mut self, local_only: bool) {
        self.routing.local_only = local_only;
    }

    /// 测试/导入用：追加既有内存历史，不做持久化。
    pub fn push_history(&mut self, message: ChatMessage) {
        self.messages.push(message);
    }

    /// 停止一次正在运行的 turn。
    pub fn stop(&self, turn: &ChatTurn) {
        turn.stop();
    }

    /// 最近一条用户消息（供命令层拿去做库内检索，PRD-16-005）。
    #[must_use]
    pub fn last_user_message(&self) -> Option<&str> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == ChatRole::User)
            .map(|m| m.content.as_str())
    }

    /// 发送一条用户消息：追加 user → 截断上下文 → 提交 `JobKind::Chat`。
    ///
    /// `grounding` = 本轮的**瞬态上下文**（库内检索摘录，PRD-16 D3 / ADR-026 ⑤）：
    /// 随本轮请求提交，**不写入 `messages`、不进归档笔记**。
    pub fn send<S: JobSink>(
        &mut self,
        user_input: impl Into<String>,
        grounding: Option<&str>,
        sink: &S,
    ) -> Result<ChatTurn, ChatSessionError> {
        let content = user_input.into();
        if content.trim().is_empty() {
            return Err(ChatSessionError::EmptyUserMessage);
        }
        self.messages
            .push(ChatMessage::new(ChatRole::User, content));
        Ok(self.submit_current_context(grounding, sink))
    }

    /// 重新生成：丢弃末条 assistant，以同一上下文重发，默认替换旧回复。
    ///
    /// 瞬态上下文**由调用方重新给**而不是缓存——「换个检索范围再答一次」正是重新生成的
    /// 主要用途之一（PRD-16-006 透明行里剔除某篇后重答），缓存住旧摘录会让那个动作失效。
    pub fn regenerate<S: JobSink>(
        &mut self,
        grounding: Option<&str>,
        sink: &S,
    ) -> Result<ChatTurn, ChatSessionError> {
        if matches!(
            self.messages.last().map(|m| m.role),
            Some(ChatRole::Assistant)
        ) {
            self.messages.pop();
        }
        if !self.messages.iter().any(|m| m.role == ChatRole::User) {
            return Err(ChatSessionError::NoUserTurn);
        }
        Ok(self.submit_current_context(grounding, sink))
    }

    fn submit_current_context<S: JobSink>(&self, grounding: Option<&str>, sink: &S) -> ChatTurn {
        let context = self.build_context(grounding);
        let mut job = Job::new(
            JobKind::Chat {
                messages: context.messages.clone(),
                tools: Vec::new(),
            },
            JobOrigin::Chat,
        );
        job.priority = Priority::Interactive;
        job.routing = self.routing.clone();
        job.options.streaming = true;
        let handle = sink.submit(job);
        ChatTurn {
            handle,
            context,
            assistant_buffer: String::new(),
        }
    }

    /// 把一轮 [`ChatTurn::drain_with`] 的结果落账到会话内存历史。
    ///
    /// 纪律与旧 `drain_into_with` 完全一致：失败不落账；完成/取消/流中断时把已产出的
    /// assistant 文本（含部分输出）追加为 assistant 消息，空文本不追加。
    pub fn absorb(&mut self, outcome: &ChatTurnOutcome) {
        if outcome.failed.is_none() && !outcome.assistant.is_empty() {
            push_assistant(self, outcome.assistant.clone());
        }
    }

    /// 组装本轮提交的 messages。
    ///
    /// 瞬态上下文以 **System 消息**注入、排在预设 system 之后与历史之前：
    /// - System 角色使它在 [`shrink_to_budget`] 里**不被丢弃**（先丢的是历史轮次）——
    ///   本轮检索到的依据比三轮之前的闲聊更该留下；
    /// - 排在历史之前而不是贴着末条 user，是为了让「指令与依据」整体靠前、稳定，
    ///   与蒸馏侧「旧内容排在最后」的排布理由相同：模型对**开头**的指令遵循最好。
    ///
    /// 预算上先给瞬态块**留位**再选历史窗口，否则会出现「历史刚好填满 → 摘录挤不进去 →
    /// 检索白做」这种静默失效。摘录本身超过半个预算时按字符截断（检索侧已有字符预算，
    /// 这里只是最后一道兜底，绝不让它把用户自己的问题挤掉）。
    fn build_context(&self, grounding: Option<&str>) -> ChatContext {
        let input_limit = self.token_budget.input_limit();
        let (system, rest) = split_system(&self.messages);

        let mut truncated = false;
        let grounding_message = grounding.map(str::trim).filter(|g| !g.is_empty()).map(|g| {
            let message = ChatMessage::new(ChatRole::System, g);
            let tokens = self
                .counter
                .count_chat_tokens(std::slice::from_ref(&message))
                .tokens;
            if tokens * 2 <= input_limit {
                return message;
            }
            truncated = true;
            let keep =
                g.chars().count() * (input_limit as usize / 2).max(1) / (tokens as usize).max(1);
            ChatMessage::new(ChatRole::System, g.chars().take(keep).collect::<String>())
        });

        let reserved = grounding_message
            .as_ref()
            .map(|m| {
                self.counter
                    .count_chat_tokens(std::slice::from_ref(m))
                    .tokens
            })
            .unwrap_or(0);
        let history_limit = input_limit.saturating_sub(reserved);

        let mut selected_rest = Vec::new();
        for start in (0..rest.len()).rev() {
            let candidate_rest = &rest[start..];
            let candidate = join_system_and_rest(system.as_ref(), candidate_rest);
            if self.counter.count_chat_tokens(&candidate).tokens <= history_limit {
                selected_rest = candidate_rest.to_vec();
            } else {
                break;
            }
        }

        while matches!(
            selected_rest.first().map(|m| m.role),
            Some(ChatRole::Assistant)
        ) {
            selected_rest.remove(0);
        }

        let mut messages = join_system_and_rest(system.as_ref(), &selected_rest);
        let original_len = self.messages.len();
        truncated |= messages.len() < original_len;
        if let Some(grounding_message) = grounding_message {
            // 插在预设 system 之后（无预设时即队首）。
            messages.insert(usize::from(system.is_some()), grounding_message);
        }
        shrink_to_budget(
            &mut messages,
            input_limit,
            self.counter.as_ref(),
            &mut truncated,
        );
        let tokens_in = self.counter.count_chat_tokens(&messages).tokens;
        ChatContext {
            messages,
            truncated,
            tokens_in,
            input_limit,
        }
    }
}

/// 正在执行的一轮 Chat Job。
pub struct ChatTurn {
    handle: JobHandle,
    context: ChatContext,
    assistant_buffer: String,
}

impl ChatTurn {
    /// 触发取消传播。
    pub fn stop(&self) {
        self.handle.cancel();
    }

    /// 克隆本轮 Job 的取消令牌。
    #[must_use]
    pub fn cancel_token(&self) -> luna_core::CancelToken {
        self.handle.cancel_token()
    }

    /// 本轮提交上下文。
    #[must_use]
    pub fn context(&self) -> &ChatContext {
        &self.context
    }

    /// 消费事件流并把最终 assistant 写回会话内存历史（不观测流式 token）。
    pub async fn drain_into(self, session: &mut ChatSession) -> ChatTurnOutcome {
        self.drain_into_with(session, |_| {}).await
    }

    /// 消费事件流并把最终 assistant 写回会话内存历史，逐 token 回调 `on_token`。
    /// = [`Self::drain_with`] + [`ChatSession::absorb`]（DRY，单一消费循环）。
    pub async fn drain_into_with<F: FnMut(&str)>(
        self,
        session: &mut ChatSession,
        on_token: F,
    ) -> ChatTurnOutcome {
        let outcome = self.drain_with(on_token).await;
        session.absorb(&outcome);
        outcome
    }

    /// 消费事件流，每到达一个流式 token 即回调 `on_token`（供 Tauri 命令把 token 经
    /// `ipc::Channel` 推给前端，落地 PRD-03-002 [P0] 流式输出 / TDD-01 §6.5）。
    ///
    /// **不写回会话**：调用方随后以 [`ChatSession::absorb`] 落账。拆开是为了让命令层在
    /// drain 的全程不持会话锁（会话存储为 `Arc<Mutex<ChatSession>>` 时，其它窗口/视图仍可
    /// 随时 attach 读取历史，TDD-01 §6.5）。
    pub async fn drain_with<F: FnMut(&str)>(mut self, mut on_token: F) -> ChatTurnOutcome {
        let mut outcome = ChatTurnOutcome {
            truncated: self.context.truncated,
            tokens_in: self.context.tokens_in,
            input_limit: self.context.input_limit,
            ..ChatTurnOutcome::default()
        };

        while let Some(event) = self.handle.recv().await {
            match event {
                JobEvent::Partial(luna_core::PartialPayload::Token(token)) => {
                    on_token(&token);
                    self.assistant_buffer.push_str(&token);
                    outcome.assistant.push_str(&token);
                }
                JobEvent::Finished(result) => {
                    let final_text = assistant_text_from_payload(result.payload)
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| self.assistant_buffer.clone());
                    if !final_text.is_empty() {
                        outcome.assistant = final_text;
                    }
                    outcome.finished = true;
                    return outcome;
                }
                JobEvent::Cancelled => {
                    outcome.cancelled = true;
                    return outcome;
                }
                JobEvent::Failed(err) => {
                    outcome.failed = Some(err);
                    return outcome;
                }
                // 对话不消费执行者事件：这条路由到的就是执行者本身。
                // （远程转写才有「路由到的 Provider ≠ 真正干活的那一方」这回事。）
                // 服务端排位同理只属于批式转写这条路（ADR-049 ⑧）：对话走的是交互车道，
                // 那边根本不排队。静默重连窗（ADR-038 ⑧）也只属于实时 ASR：对话是
                // 一次请求一次回答，断了就是断了，没有「接着说」这回事。
                // 本机准入等待（ADR-078 ③）同样与对话无关：它只发给会话池的 Job
                // （流式 ASR 腿 / 实时转译），而对话走的是计算池。
                JobEvent::Queued
                | JobEvent::AdmissionDelayed { .. }
                | JobEvent::Started
                | JobEvent::Progress(_)
                | JobEvent::Usage(_)
                | JobEvent::ExecutedBy(_)
                | JobEvent::RemoteQueued(_)
                | JobEvent::Reconnecting(_)
                | JobEvent::Partial(luna_core::PartialPayload::Asr { .. }) => {}
            }
        }

        if !self.assistant_buffer.is_empty() {
            outcome.assistant = self.assistant_buffer;
        }
        outcome
    }
}

/// 一轮 Chat 的消费结果。
#[derive(Clone, Debug, Default)]
pub struct ChatTurnOutcome {
    /// 是否发生上下文截断。
    pub truncated: bool,
    /// 输入 token 数。
    pub tokens_in: u32,
    /// 输入预算。
    pub input_limit: u32,
    /// 本轮 assistant 文本。
    pub assistant: String,
    /// 是否正常完成。
    pub finished: bool,
    /// 是否取消。
    pub cancelled: bool,
    /// 失败原因。
    pub failed: Option<JobError>,
}

fn split_system(messages: &[ChatMessage]) -> (Option<ChatMessage>, &[ChatMessage]) {
    match messages.first() {
        Some(first) if first.role == ChatRole::System => (Some(first.clone()), &messages[1..]),
        _ => (None, messages),
    }
}

fn join_system_and_rest(system: Option<&ChatMessage>, rest: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut out = Vec::with_capacity(rest.len() + usize::from(system.is_some()));
    if let Some(system) = system {
        out.push(system.clone());
    }
    out.extend_from_slice(rest);
    out
}

fn shrink_to_budget(
    messages: &mut Vec<ChatMessage>,
    input_limit: u32,
    counter: &dyn TokenCounter,
    truncated: &mut bool,
) {
    while counter.count_chat_tokens(messages).tokens > input_limit && !messages.is_empty() {
        if let Some(index) = messages.iter().position(|m| m.role != ChatRole::System) {
            messages.remove(index);
            *truncated = true;
        } else {
            let system = &mut messages[0].content;
            if system.pop().is_none() {
                break;
            }
            *truncated = true;
        }
    }
}

fn assistant_text_from_payload(payload: ResultPayload) -> Option<String> {
    match payload {
        ResultPayload::Text(text) => Some(text),
        ResultPayload::Chat(message) => Some(message.content),
        // 对话拿不到文本的两种载荷：段序列（转写）与向量（嵌入）。都不是回复，返回 None
        // 让调用方走「引擎没给出回复」那条诚实路径，而不是编一个空字符串。
        ResultPayload::Segments(_) | ResultPayload::Embeddings(_) => None,
    }
}

fn push_assistant(session: &mut ChatSession, text: String) {
    session
        .messages
        .push(ChatMessage::new(ChatRole::Assistant, text));
}
