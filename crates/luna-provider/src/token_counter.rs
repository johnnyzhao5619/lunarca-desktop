//! Provider 级 chat token 计数接缝（TDD-03 §3.3）。
//!
//! 精确 tokenizer 接入后应实现同一 trait；未知模型先走保守启发式，供 ChatSession
//! 在提交 `JobKind::Chat` 前裁剪上下文，避免撞上引擎的超窗拒绝（或外部端点的静默截断）。

use luna_core::{ChatMessage, ChatRole};

/// Token 计数方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenCountMethod {
    /// 已登记模型的精确 tokenizer。
    Exact,
    /// 未知模型的保守启发式。
    Heuristic,
}

/// Chat messages 的计数结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenCount {
    /// 估计或精确 token 数。
    pub tokens: u32,
    /// 计数方式。
    pub method: TokenCountMethod,
    /// 是否已经计入 chat template / role 包裹开销。
    pub includes_chat_template: bool,
}

/// Chat 上下文预算。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenBudget {
    /// 模型上下文窗口（`n_ctx`）。
    pub context_window: u32,
    /// 为输出预留的 token。
    pub reserved_output_tokens: u32,
}

impl TokenBudget {
    /// 可用于输入 messages 的预算，永不下溢。
    #[must_use]
    pub fn input_limit(self) -> u32 {
        self.context_window
            .saturating_sub(self.reserved_output_tokens)
    }
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self {
            context_window: 4096,
            reserved_output_tokens: 512,
        }
    }
}

/// Provider 级 token counter。
pub trait TokenCounter: Send + Sync {
    /// 计算 chat messages 的输入 token 数。
    fn count_chat_tokens(&self, messages: &[ChatMessage]) -> TokenCount;
}

/// 未知模型兜底：按字符类别估算 + 固定 chat template 包裹 + 安全余量。
#[derive(Clone, Debug)]
pub struct HeuristicTokenCounter {
    per_message_overhead: u32,
    assistant_primer_overhead: u32,
    safety_margin_tokens: u32,
}

impl Default for HeuristicTokenCounter {
    fn default() -> Self {
        Self {
            per_message_overhead: 6,
            assistant_primer_overhead: 4,
            safety_margin_tokens: 32,
        }
    }
}

impl HeuristicTokenCounter {
    /// 构造默认启发式计数器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl TokenCounter for HeuristicTokenCounter {
    fn count_chat_tokens(&self, messages: &[ChatMessage]) -> TokenCount {
        let body_tokens: u32 = messages
            .iter()
            .map(|m| {
                self.per_message_overhead
                    + role_overhead(m.role)
                    + conservative_text_tokens(&m.content)
            })
            .sum();
        TokenCount {
            tokens: body_tokens + self.assistant_primer_overhead + self.safety_margin_tokens,
            method: TokenCountMethod::Heuristic,
            includes_chat_template: true,
        }
    }
}

fn role_overhead(role: ChatRole) -> u32 {
    match role {
        ChatRole::System => 2,
        ChatRole::User | ChatRole::Assistant => 1,
    }
}

fn conservative_text_tokens(text: &str) -> u32 {
    let mut cjk = 0_u32;
    let mut other = 0_u32;
    for ch in text.chars() {
        if is_cjk(ch) {
            cjk += 1;
        } else if !ch.is_whitespace() {
            other += 1;
        }
    }
    let latinish = other.div_ceil(3);
    (cjk + latinish).max(1)
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x3040..=0x30FF
            | 0xAC00..=0xD7AF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_input_limit_saturates() {
        assert_eq!(
            TokenBudget {
                context_window: 128,
                reserved_output_tokens: 32
            }
            .input_limit(),
            96
        );
        assert_eq!(
            TokenBudget {
                context_window: 32,
                reserved_output_tokens: 128
            }
            .input_limit(),
            0
        );
    }

    #[test]
    fn heuristic_counts_template_overhead() {
        let counter = HeuristicTokenCounter::new();
        let count = counter.count_chat_tokens(&[
            ChatMessage::new(ChatRole::System, "rules"),
            ChatMessage::new(ChatRole::User, "hello 世界"),
        ]);
        assert!(count.tokens > 32);
        assert_eq!(count.method, TokenCountMethod::Heuristic);
        assert!(count.includes_chat_template);
    }
}
