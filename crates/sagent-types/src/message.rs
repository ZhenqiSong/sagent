//! 从 SQLite 读取后供 CLI、Runtime 和协议层共享的消息 DTO。

use serde::{Deserialize, Serialize};

use crate::{MessageId, SessionId};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredMessage {
    /// SQLite messages.id，对外保留强类型边界。
    pub id: MessageId,
    /// 消息所属的会话。
    pub session_id: SessionId,
    /// 逻辑角色，例如 user、assistant 或 tool。
    pub role: String,
    /// 消息正文；工具结果也通过此字段进入模型上下文。
    pub content: String,
    /// 数据库记录的创建时间。
    pub timestamp: Option<String>,
    /// 工具结果关联的 Provider call id。
    pub tool_call_id: Option<String>,
    /// 工具名称；普通消息为空。
    pub tool_name: Option<String>,
    /// assistant 发起的工具调用 JSON。
    pub tool_calls: Option<String>,
    /// Provider 返回的推理文本（如有）。
    pub reasoning: Option<String>,
    /// Provider 的结束原因。
    pub finish_reason: Option<String>,
    /// UI 展示类型。
    pub display_kind: Option<String>,
    /// UI 展示所需的附加 JSON 元数据。
    pub display_metadata: Option<String>,
    /// 是否属于当前可见消息链。
    pub active: bool,
    /// 是否已被压缩归档。
    pub compacted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchHit {
    /// 命中消息所属的会话。
    pub session_id: SessionId,
    /// FTS 行关联的消息 ID；未来会话级搜索可不提供此值。
    pub message_id: Option<MessageId>,
    /// 由 SQLite FTS5 生成的、带命中标记的上下文片段。
    pub snippet: String,
    /// FTS5 bm25 相关度；数值越小表示匹配越相关。
    pub rank: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::StoredMessage;
    use serde_json::{Value, json};

    /// 消息 DTO 必须保留工具调用、推理和压缩状态，供后续会话恢复与 TUI 渲染使用。
    #[test]
    fn stored_message_round_trips_all_persisted_fields() {
        let expected = json!({
            "id": 7,
            "session_id": "20260829_123000_abcdefgh",
            "role": "assistant",
            "content": "我会先检查数据库结构。",
            "timestamp": "2026-08-29T12:31:00Z",
            "tool_call_id": "call_123",
            "tool_name": "schema.inspect",
            "tool_calls": "[{\"name\":\"schema.inspect\"}]",
            "reasoning": "需要先确认表结构。",
            "finish_reason": "tool_calls",
            "display_kind": "tool_call",
            "display_metadata": "{\"collapsed\":true}",
            "active": true,
            "compacted": false
        });

        let message: StoredMessage =
            serde_json::from_value(expected.clone()).expect("存储消息应能反序列化");

        let actual: Value = serde_json::to_value(message).expect("存储消息应能序列化");
        assert_eq!(actual, expected);
    }

    /// null 表示数据库中没有该可选元数据，不能被错误地改写为空字符串。
    #[test]
    fn stored_message_preserves_absent_optional_metadata() {
        let input = json!({
            "id": 8,
            "session_id": "20260829_123000_abcdefgh",
            "role": "user",
            "content": "继续",
            "timestamp": null,
            "tool_call_id": null,
            "tool_name": null,
            "tool_calls": null,
            "reasoning": null,
            "finish_reason": null,
            "display_kind": null,
            "display_metadata": null,
            "active": true,
            "compacted": false
        });

        let message: StoredMessage =
            serde_json::from_value(input.clone()).expect("可选字段为 null 时应能反序列化");

        let actual: Value = serde_json::to_value(message).expect("存储消息应能序列化");
        assert_eq!(actual, input);
    }
}
