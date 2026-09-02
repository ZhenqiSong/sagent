//! Agent 领域命令。
//!
//! 命令表示外部希望回合执行的动作；它们不直接执行数据库或网络操作，
//! 而是交给后续的状态机/Runtime 解释。

use sagent_types::{ApprovalId, ClientCapabilities};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// 构造领域命令时可能出现的错误。
#[derive(Debug, Clone, Eq, Error, PartialEq)]
pub enum CommandError {
    /// 用户没有提供有意义的文本。
    #[error("用户输入不能为空")]
    EmptyInput,
}

/// 用户对工具审批请求作出的决定。
///
/// 超时不是用户决定，而由 Runtime 通过 `TurnEvent::ApprovalTimedOut` 表示。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// 仅允许当前工具调用。
    Once,
    /// 当前会话内允许同类调用。
    Session,
    /// 后续始终允许同类调用。
    Always,
    /// 拒绝本次调用。
    Deny,
}

/// 一次客户端请求的领域标识。
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(Uuid);

impl RequestId {
    /// 生成请求标识。
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

/// 用户提交给 Agent 的文本输入。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserInput {
    text: String,
}

impl UserInput {
    /// 创建输入；空白输入被拒绝，避免产生无意义回合。
    pub fn new(text: impl Into<String>) -> Result<Self, CommandError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(CommandError::EmptyInput);
        }
        Ok(Self { text })
    }

    /// 返回用户原始输入文本。
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

/// SessionActor 可以接收的领域命令。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum SessionCommand {
    /// 提交一个新的用户回合。
    SubmitPrompt {
        request_id: RequestId,
        input: UserInput,
    },
    /// 请求中断当前回合。
    Interrupt { request_id: RequestId },
    /// 响应待处理的工具审批。
    ResolveApproval {
        approval_id: ApprovalId,
        decision: ApprovalDecision,
    },
    /// 让客户端声明能力并恢复会话交互。
    Resume { client: ClientCapabilities },
    /// 关闭会话运行时。
    Close,
}

#[cfg(test)]
mod tests {
    use super::{ApprovalDecision, CommandError, RequestId, SessionCommand, UserInput};

    #[test]
    fn user_input_rejects_blank_text() {
        assert_eq!(UserInput::new("  \n"), Err(CommandError::EmptyInput));
        assert_eq!(
            UserInput::new(" hello ").expect("有效输入").as_str(),
            " hello "
        );
    }

    #[test]
    fn command_round_trips_as_json() {
        let command = SessionCommand::SubmitPrompt {
            request_id: RequestId::new(),
            input: UserInput::new("你好").expect("有效输入"),
        };
        let encoded = serde_json::to_string(&command).expect("命令应能序列化");
        let decoded: SessionCommand = serde_json::from_str(&encoded).expect("命令应能反序列化");
        assert_eq!(command, decoded);
    }

    #[test]
    fn approval_decision_uses_named_policy() {
        let command = SessionCommand::ResolveApproval {
            approval_id: sagent_types::ApprovalId::new(),
            decision: ApprovalDecision::Session,
        };
        let value = serde_json::to_value(&command).expect("审批命令应能序列化");
        assert_eq!(value["ResolveApproval"]["decision"], "session");
    }
}
