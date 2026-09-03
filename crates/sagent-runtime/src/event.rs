//! Runtime 对外发布的事件。

use sagent_agent::RequestId;
use sagent_types::{MessageId, SessionId, TurnId};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::broadcast;

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

/// 事件订阅在 Actor 停止后返回的错误。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Error)]
pub enum SubscriptionError {
    /// Actor 已退出，后续不会再产生实时事件。
    #[error("session actor has stopped")]
    ActorStopped,
}

/// 一个 Session 的实时事件订阅。
///
/// 订阅者落后导致 broadcast 丢失消息时，不把底层 `Lagged` 错误直接暴露给
/// RPC/TUI，而是返回一个带 skipped 数量的 `SubscriberLagged` 诊断事件；
/// 客户端随后可以用 Store::events_since 补读持久化事实。
pub struct RuntimeEventSubscription {
    session_id: SessionId,
    receiver: broadcast::Receiver<RuntimeEvent>,
}

impl RuntimeEventSubscription {
    pub(crate) fn new(session_id: SessionId, receiver: broadcast::Receiver<RuntimeEvent>) -> Self {
        Self {
            session_id,
            receiver,
        }
    }

    /// 返回此订阅绑定的会话。
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// 等待下一条事件；lag 会转换为可处理的诊断事件。
    pub async fn recv(&mut self) -> Result<RuntimeEvent, SubscriptionError> {
        match self.receiver.recv().await {
            Ok(event) => Ok(event),
            Err(broadcast::error::RecvError::Lagged(skipped)) => Ok(self.lagged(skipped)),
            Err(broadcast::error::RecvError::Closed) => Err(SubscriptionError::ActorStopped),
        }
    }

    /// 非阻塞读取下一条事件；暂时没有事件时返回 None。
    pub fn try_recv(&mut self) -> Result<Option<RuntimeEvent>, SubscriptionError> {
        match self.receiver.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(broadcast::error::TryRecvError::Empty) => Ok(None),
            Err(broadcast::error::TryRecvError::Lagged(skipped)) => Ok(Some(self.lagged(skipped))),
            Err(broadcast::error::TryRecvError::Closed) => Err(SubscriptionError::ActorStopped),
        }
    }

    fn lagged(&self, skipped: u64) -> RuntimeEvent {
        RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: None,
            request_id: None,
            kind: RuntimeEventKind::SubscriberLagged { skipped },
        }
    }
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
    use tokio::sync::broadcast;

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

    #[tokio::test]
    async fn slow_subscriber_receives_a_lag_diagnostic() {
        let session_id = SessionId::new("lagged-session");
        let (sender, receiver) = broadcast::channel(2);
        let mut subscription = super::RuntimeEventSubscription::new(session_id.clone(), receiver);

        for text in ["one", "two", "three"] {
            sender
                .send(RuntimeEvent {
                    session_id: session_id.clone(),
                    turn_id: None,
                    request_id: None,
                    kind: RuntimeEventKind::ModelTextDelta { text: text.into() },
                })
                .expect("仍应有订阅者");
        }

        let event = subscription.recv().await.expect("lag 不应关闭订阅");
        assert!(matches!(
            event.kind,
            RuntimeEventKind::SubscriberLagged { skipped: 1 }
        ));
    }

    #[tokio::test]
    async fn closed_subscriber_reports_actor_stopped() {
        let (sender, receiver) = broadcast::channel(2);
        let mut subscription =
            super::RuntimeEventSubscription::new(SessionId::new("closed-session"), receiver);
        drop(sender);

        assert_eq!(
            subscription.recv().await,
            Err(super::SubscriptionError::ActorStopped)
        );
    }
}
