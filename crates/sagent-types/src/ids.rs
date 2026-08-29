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

/// 持久化消息标识；与 `SessionId` 保持不同类型以防止参数错传。
#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(String);

#[cfg(test)]
mod tests {
    use super::{MessageId, SessionId};

    #[test]
    fn session_id_serializes_as_a_plain_json_string() {
        let id = SessionId("20260829_123000_abcdefgh".to_owned());

        assert_eq!(
            serde_json::to_string(&id).expect("应能序列化"),
            "\"20260829_123000_abcdefgh\""
        );
    }

    #[test]
    fn message_id_deserializes_from_a_plain_json_string() {
        let id: MessageId = serde_json::from_str("\"message-1\"").expect("应能反序列化");

        assert_eq!(id.0, "message-1");
    }
}
