//! 对话消息与工具声明（零 IO）。被 `JobKind::Chat` 与 `ResultPayload::Chat` 共用。

use serde::{Deserialize, Serialize};

/// 对话角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    /// 系统/角色预设（PromptPreset.system_prompt，TDD-01 §2.5）。
    System,
    /// 用户。
    User,
    /// 助手回复。
    Assistant,
}

/// 一条对话消息。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// 角色。
    pub role: ChatRole,
    /// 文本内容。
    pub content: String,
}

impl ChatMessage {
    /// 便捷构造。
    #[must_use]
    pub fn new(role: ChatRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }
}

/// 工具调用声明（占位）。MVP 工具能力弱，但提示注入护栏（白名单 + 用户确认）
/// 见 TDD-12 §4。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// 工具名。
    pub name: String,
    /// 用途描述。
    pub description: String,
}
