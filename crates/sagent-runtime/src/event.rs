//! Runtime 对外发布的事件。

use sagent_agent::RequestId;
use sagent_types::{MessageId, SessionId, TurnId};
use serde::{Deserialize, Serialize};

/// Runtime 发送给未来 RPC/TUI 客户端的事件。
///
/// 事件携带 Session/Turn/Request 关联信息，避免多个会话并行时发生串线。
/// 其中 delta 是瞬态事件；持久化确认类事件只会在 Store 提交成功后发布。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    #[serde(flatten)]
    pub kind: RuntimeEventKind,
}

/// Runtime 事件的具体种类。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum RuntimeEventKind {
    /// prompt 已通过 busy 检查并完成初步接受。
    PromptAccepted,
    /// user message 已由 Store 提交。
    UserMessagePersisted {
        message_id: MessageId,
    },
    /// 模型流式输出片段；不写入 daemon_events。
    ModelTextDelta {
        text: String,
    },
    /// assistant 最终消息已经持久化。
    FinalMessagePersisted {
        message_id: MessageId,
    },
    /// Turn 已完成。
    TurnCompleted,
    /// Turn 已中断。
    TurnInterrupted,
    /// Turn 因可控错误失败。
    TurnFailed {
        reason: String,
    },
    /// actor 启动/停止诊断。
    ActorStarted,
    ActorStopped,
    /// 广播订阅者因处理过慢而丢失了瞬态事件。
    SubscriberLagged {
        skipped: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::{RuntimeEvent, RuntimeEventKind};
    use sagent_types::{SessionId, TurnId};

    #[test]
    fn event_round_trips_with_session_and_turn_context() {
        let event = RuntimeEvent {
            session_id: SessionId::new("session-1"),
            turn_id: Some(TurnId::new()),
            request_id: None,
            kind: RuntimeEventKind::ModelTextDelta {
                text: "你好".into(),
            },
        };

        let encoded = serde_json::to_string(&event).expect("事件应能序列化");
        let decoded: RuntimeEvent = serde_json::from_str(&encoded).expect("事件应能反序列化");

        assert_eq!(event, decoded);
    }

    #[test]
    fn transient_delta_has_stable_wire_name() {
        let event = RuntimeEvent {
            session_id: SessionId::new("session-1"),
            turn_id: None,
            request_id: None,
            kind: RuntimeEventKind::ModelTextDelta {
                text: "chunk".into(),
            },
        };

        let value = serde_json::to_value(event).expect("事件应能转换为 JSON");

        assert_eq!(value["type"], "model_text_delta");
        assert_eq!(value["data"]["text"], "chunk");
    }
}
