//! 强类型 ID 模块。
//!
//! 提供 SessionId、TurnId、MessageId、ToolCallId 等 newtype ID，
//! 防止不同类型 ID 互相传递导致逻辑错误。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 基础 ID 类型

use serde::{Deserialize, Serialize};

/// Session 唯一标识符
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

/// Turn 唯一标识符（一次 Agent 交互轮次）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId(pub String);

/// 消息唯一标识符
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

/// 工具调用唯一标识符
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolCallId(pub String);

/// 事件唯一标识符
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventId(pub String);

/// 请求唯一标识符（对应 JSON-RPC request id）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// 字符串类型 ID
    String(String),
    /// 数字类型 ID
    Number(i64),
}
