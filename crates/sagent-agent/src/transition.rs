//! 回合状态转换规则。
//!
//! 该模块只做纯判断，不执行副作用。数据库写入、模型调用和工具执行由后续
//! Runtime 根据转换结果完成。

use crate::state::TurnState;
use crate::{ApprovalDecision, SessionCommand, TurnEvent};
use thiserror::Error;

/// 执行命令时的状态转换错误。
#[derive(Debug, Clone, Eq, Error, PartialEq)]
pub enum TransitionError {
    /// 当前状态不允许提交新命令。
    #[error("状态 {state:?} 不允许执行命令 {command:?}")]
    InvalidCommand {
        state: TurnState,
        command: SessionCommand,
    },
}

/// 根据当前状态和领域命令计算下一状态。
///
/// 该函数是纯函数：相同输入永远得到相同结果，也不会修改数据库或调用外部服务。
pub fn apply_command(
    state: TurnState,
    command: &SessionCommand,
) -> Result<TurnState, TransitionError> {
    let next = match (state, command) {
        (TurnState::Idle, SessionCommand::SubmitPrompt { .. }) => TurnState::Prompting,
        (TurnState::Idle, SessionCommand::Resume { .. }) => TurnState::Idle,

        (TurnState::Prompting, SessionCommand::Interrupt { .. })
        | (TurnState::AwaitingModel, SessionCommand::Interrupt { .. })
        | (TurnState::RunningTool, SessionCommand::Interrupt { .. })
        | (TurnState::AwaitingApproval, SessionCommand::Interrupt { .. }) => TurnState::Interrupted,

        (TurnState::AwaitingApproval, SessionCommand::ResolveApproval { decision, .. }) => {
            match decision {
                ApprovalDecision::Once | ApprovalDecision::Session | ApprovalDecision::Always => {
                    TurnState::RunningTool
                }
                ApprovalDecision::Deny => TurnState::Failed,
            }
        }

        // Close 关闭的是 SessionActor，而不是当前回合；TurnState 保持不变。
        (_, SessionCommand::Close) => state,

        _ => {
            return Err(TransitionError::InvalidCommand {
                state,
                command: command.clone(),
            });
        }
    };

    Ok(next)
}

/// 根据已经发生的领域事件推进状态。
pub fn apply_event(state: TurnState, event: &TurnEvent) -> Result<TurnState, TransitionError> {
    let next = match (state, event) {
        (TurnState::Idle, TurnEvent::PromptAccepted { .. }) => TurnState::Prompting,
        (TurnState::Prompting, TurnEvent::UserMessagePersisted { .. }) => TurnState::Prompting,
        (TurnState::Prompting, TurnEvent::PromptSnapshotReady { .. }) => TurnState::AwaitingModel,
        (TurnState::AwaitingModel, TurnEvent::ModelTextDelta { .. }) => TurnState::AwaitingModel,
        (TurnState::AwaitingModel, TurnEvent::ToolCallRequested { .. }) => TurnState::RunningTool,
        (TurnState::RunningTool, TurnEvent::ApprovalRequested { .. }) => {
            TurnState::AwaitingApproval
        }
        (TurnState::AwaitingApproval, TurnEvent::ApprovalResolved { decision, .. }) => {
            match decision {
                ApprovalDecision::Once | ApprovalDecision::Session | ApprovalDecision::Always => {
                    TurnState::RunningTool
                }
                ApprovalDecision::Deny => TurnState::Failed,
            }
        }
        (TurnState::AwaitingApproval, TurnEvent::ApprovalTimedOut { .. }) => TurnState::Failed,
        (TurnState::RunningTool, TurnEvent::ToolResultReady { .. }) => TurnState::AwaitingModel,
        (TurnState::AwaitingModel, TurnEvent::FinalMessagePersisted { .. }) => TurnState::Completed,
        (TurnState::Completed, TurnEvent::OutcomePersisted { .. }) => TurnState::Completed,
        (
            TurnState::Prompting
            | TurnState::AwaitingModel
            | TurnState::RunningTool
            | TurnState::AwaitingApproval,
            TurnEvent::Interrupted { .. },
        ) => TurnState::Interrupted,
        (
            TurnState::Prompting
            | TurnState::AwaitingModel
            | TurnState::RunningTool
            | TurnState::AwaitingApproval,
            TurnEvent::Failed { .. },
        ) => TurnState::Failed,
        _ => {
            return Err(TransitionError::InvalidCommand {
                state,
                command: SessionCommand::Close,
            });
        }
    };
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::{TransitionError, apply_command, apply_event};
    use crate::TurnState;
    use crate::{ApprovalDecision, RequestId, SessionCommand, TurnEvent, UserInput};
    use sagent_types::{ApprovalId, ClientCapabilities};

    fn submit() -> SessionCommand {
        SessionCommand::SubmitPrompt {
            request_id: RequestId::new(),
            input: UserInput::new("你好").expect("测试输入有效"),
        }
    }

    #[test]
    fn submit_starts_prompting_from_idle() {
        assert_eq!(
            apply_command(TurnState::Idle, &submit()),
            Ok(TurnState::Prompting)
        );
    }

    #[test]
    fn approval_policy_controls_next_state() {
        let once = SessionCommand::ResolveApproval {
            approval_id: ApprovalId::new(),
            decision: ApprovalDecision::Once,
        };
        let deny = SessionCommand::ResolveApproval {
            approval_id: ApprovalId::new(),
            decision: ApprovalDecision::Deny,
        };

        assert_eq!(
            apply_command(TurnState::AwaitingApproval, &once),
            Ok(TurnState::RunningTool)
        );
        assert_eq!(
            apply_command(TurnState::AwaitingApproval, &deny),
            Ok(TurnState::Failed)
        );
    }

    #[test]
    fn terminal_states_reject_new_prompt() {
        let command = submit();
        for state in [
            TurnState::Completed,
            TurnState::Interrupted,
            TurnState::Failed,
        ] {
            assert!(matches!(
                apply_command(state, &command),
                Err(TransitionError::InvalidCommand { .. })
            ));
        }
    }

    #[test]
    fn approval_can_only_be_resolved_while_waiting() {
        let command = SessionCommand::ResolveApproval {
            approval_id: ApprovalId::new(),
            decision: ApprovalDecision::Always,
        };
        assert!(apply_command(TurnState::RunningTool, &command).is_err());
    }

    #[test]
    fn resume_is_a_handshake_and_close_preserves_turn_state() {
        let resume = SessionCommand::Resume {
            client: ClientCapabilities {
                client_id: sagent_types::ClientId::new(),
                surface: sagent_types::ClientSurface::Tui,
                interactive_approval: true,
                supports_stream_edits: true,
                protocol_version: 1,
            },
        };
        assert_eq!(apply_command(TurnState::Idle, &resume), Ok(TurnState::Idle));

        assert_eq!(
            apply_command(TurnState::AwaitingModel, &SessionCommand::Close),
            Ok(TurnState::AwaitingModel)
        );
    }

    #[test]
    fn events_advance_the_turn_lifecycle() {
        let turn_id = sagent_types::TurnId::new();
        let state = apply_event(TurnState::Idle, &TurnEvent::PromptAccepted { turn_id }).unwrap();
        let state = apply_event(
            state,
            &TurnEvent::PromptSnapshotReady {
                turn_id,
                hash: "h".into(),
            },
        )
        .unwrap();
        let state = apply_event(
            state,
            &TurnEvent::ToolCallRequested {
                turn_id,
                tool_call_id: sagent_types::ToolCallId::new(),
            },
        )
        .unwrap();
        assert_eq!(state, TurnState::RunningTool);
    }

    #[test]
    fn timeout_is_terminal_and_completed_cannot_be_interrupted() {
        let turn_id = sagent_types::TurnId::new();
        let approval_id = sagent_types::ApprovalId::new();
        assert_eq!(
            apply_event(
                TurnState::AwaitingApproval,
                &TurnEvent::ApprovalTimedOut {
                    turn_id,
                    approval_id
                }
            ),
            Ok(TurnState::Failed)
        );
        assert!(apply_event(TurnState::Completed, &TurnEvent::Interrupted { turn_id }).is_err());
    }
}
