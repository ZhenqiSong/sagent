//! Agent 回合产生的领域事件。

use crate::command::ApprovalDecision;
use crate::state::TurnFailure;
use sagent_types::{ApprovalId, MessageId, ToolCallId, TurnId};
use serde::{Deserialize, Serialize};

/// SessionActor 对外发布的事实性事件。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum TurnEvent {
    PromptAccepted {
        turn_id: TurnId,
    },
    UserMessagePersisted {
        turn_id: TurnId,
        message_id: MessageId,
    },
    PromptSnapshotReady {
        turn_id: TurnId,
        hash: String,
    },
    ModelTextDelta {
        turn_id: TurnId,
        text: String,
    },
    ToolCallRequested {
        turn_id: TurnId,
        tool_call_id: ToolCallId,
    },
    ApprovalRequested {
        turn_id: TurnId,
        approval_id: ApprovalId,
    },
    ApprovalResolved {
        turn_id: TurnId,
        approval_id: ApprovalId,
        decision: ApprovalDecision,
    },
    /// 审批在规定时间内没有得到用户响应。
    ApprovalTimedOut {
        turn_id: TurnId,
        approval_id: ApprovalId,
    },
    ToolResultReady {
        turn_id: TurnId,
        tool_call_id: ToolCallId,
    },
    FinalMessagePersisted {
        turn_id: TurnId,
        message_id: MessageId,
    },
    OutcomePersisted {
        turn_id: TurnId,
    },
    Interrupted {
        turn_id: TurnId,
    },
    Failed {
        turn_id: TurnId,
        failure: TurnFailure,
    },
}

#[cfg(test)]
mod tests {
    use super::TurnEvent;
    use sagent_types::TurnId;

    #[test]
    fn event_round_trips_as_json() {
        let event = TurnEvent::ModelTextDelta {
            turn_id: TurnId::new(),
            text: "片段".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: TurnEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, decoded);
    }
}
