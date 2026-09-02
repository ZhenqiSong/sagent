//! 跨 crate 使用的持久化标识类型。
//!
//! 作者：SongZQ
//! 创建日期：2026-08-29
//! 变更记录：
//! - 2026-08-29：定义透明 JSON 序列化的会话与消息标识。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 持久化会话标识。
///
/// 新类型避免将 session ID 与普通字符串或其他 ID 类型混用。`transparent` 使其在线上
/// JSON 协议中保持字符串形式，而不是额外包一层对象。
#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// 用数据库或外部协议中的会话标识创建强类型 ID。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 返回用于 SQL 参数绑定、日志或显示的原始字符串。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 持久化消息标识；与 `SessionId` 保持不同类型以防止参数错传。
#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(i64);

impl MessageId {
    /// 用 SQLite messages.id 创建强类型消息标识。
    pub fn new(value: i64) -> Self {
        Self(value)
    }

    /// 返回用于 SQL 参数绑定的整数主键。
    pub fn get(&self) -> i64 {
        self.0
    }
}

/// 一次 Agent 回合的内部标识。
///
/// 该标识与会话 ID 分离，便于 Runtime 将 provider、工具和取消事件关联到同一回合。
/// `transparent` 保证协议边界上仍是普通 UUID 字符串。
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TurnId(Uuid);

impl TurnId {
    /// 生成一个新的回合标识。
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// 从外部字符串解析回合标识。
    pub fn parse(value: &str) -> Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse_str(value)?))
    }

    /// 返回内部 UUID 引用。
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for TurnId {
    fn default() -> Self {
        Self::new()
    }
}

/// 等待用户决定的 approval 请求标识。
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApprovalId(Uuid);

impl ApprovalId {
    /// 生成新的 approval 标识。
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// 从外部字符串解析 approval 标识。
    pub fn parse(value: &str) -> Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse_str(value)?))
    }

    /// 返回内部 UUID 引用。
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for ApprovalId {
    fn default() -> Self {
        Self::new()
    }
}

/// 一个客户端连接实例的标识。
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientId(Uuid);

impl ClientId {
    /// 生成新的客户端标识。
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// 从外部字符串解析客户端标识。
    pub fn parse(value: &str) -> Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse_str(value)?))
    }

    /// 返回内部 UUID 引用。
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for ClientId {
    fn default() -> Self {
        Self::new()
    }
}

/// 一次工具调用的标识，用于把模型发出的调用与工具结果关联起来。
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(Uuid);

impl ToolCallId {
    /// 生成新的工具调用标识。
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// 从外部字符串解析工具调用标识。
    pub fn parse(value: &str) -> Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse_str(value)?))
    }

    /// 返回内部 UUID 引用。
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for ToolCallId {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{ApprovalId, ClientId, MessageId, SessionId, ToolCallId, TurnId};

    #[test]
    fn session_id_serializes_as_a_plain_json_string() {
        let id = SessionId::new("20260829_123000_abcdefgh");

        assert_eq!(
            serde_json::to_string(&id).expect("应能序列化"),
            "\"20260829_123000_abcdefgh\""
        );
    }

    #[test]
    fn session_id_exposes_its_sql_value() {
        let id = SessionId::new("session-1");

        assert_eq!(id.as_str(), "session-1");
    }

    #[test]
    fn message_id_deserializes_from_a_plain_json_number() {
        let id: MessageId = serde_json::from_str("7").expect("应能反序列化");

        assert_eq!(id.get(), 7);
    }

    #[test]
    fn runtime_ids_are_plain_uuid_strings_and_are_type_distinct() {
        let turn = TurnId::new();
        let approval = ApprovalId::new();
        let client = ClientId::new();
        let tool_call = ToolCallId::new();

        assert!(
            serde_json::to_value(turn)
                .expect("回合 ID 应能序列化")
                .is_string()
        );
        assert!(
            serde_json::to_value(approval)
                .expect("approval ID 应能序列化")
                .is_string()
        );
        assert!(
            serde_json::to_value(client)
                .expect("客户端 ID 应能序列化")
                .is_string()
        );
        assert!(
            serde_json::to_value(tool_call)
                .expect("工具调用 ID 应能序列化")
                .is_string()
        );
    }

    #[test]
    fn runtime_ids_round_trip_through_uuid_text() {
        let id = TurnId::new();
        let text = serde_json::to_string(&id).expect("应能序列化");
        let decoded: TurnId = serde_json::from_str(&text).expect("应能反序列化");

        assert_eq!(id, decoded);
    }
}
