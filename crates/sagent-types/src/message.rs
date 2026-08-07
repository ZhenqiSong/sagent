//! 消息类型模块。
//!
//! 定义消息角色、内容部分和完整的 Message 结构。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 消息类型定义

use serde::{Deserialize, Serialize};

use crate::ids::{MessageId, ToolCallId};
use crate::tool::ToolCall;

/// 消息角色枚举。
///
/// 对应 OpenAI Chat Completions 的四类角色。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// 系统消息（system prompt）
    System,
    /// 用户消息
    User,
    /// 助手消息（模型回复）
    Assistant,
    /// 工具执行结果消息
    Tool,
}

/// 消息内容部分。
///
/// 支持可扩展的 content parts 模型，Phase 0 只实现 text part。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// 纯文本内容
    Text { text: String },
}

/// 对话消息。
///
/// 包含消息 ID、角色、内容、工具调用和工具调用 ID 关联。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// 消息唯一标识
    pub message_id: MessageId,
    /// 消息角色
    pub role: Role,
    /// 消息内容（可包含多个 content part）
    pub content: Vec<ContentPart>,
    /// 工具调用列表（仅 assistant 消息）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// 工具调用 ID（仅 tool 消息，关联到对应的 assistant tool_call）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<ToolCallId>,
    /// 消息创建时间（RFC 3339 UTC）
    pub created_at: String,
}
