//! 跨 crate 使用的持久化标识类型。
//!
//! 作者：SongZQ
//! 创建日期：2026-08-29
//! 变更记录：
//! - 2026-08-29：定义透明 JSON 序列化的会话与消息标识。

use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::{MessageId, SessionId};

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
}
