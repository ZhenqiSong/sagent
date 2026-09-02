//! 回合状态和值域失败类型。

use serde::{Deserialize, Serialize};

/// 一个 Agent 回合的生命周期状态。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    /// 尚未接受用户输入。
    Idle,
    /// 已接受输入，正在构造提示词。
    Prompting,
    /// 正在等待模型输出。
    AwaitingModel,
    /// 正在执行工具调用。
    RunningTool,
    /// 等待用户对高风险工具作出决定。
    AwaitingApproval,
    /// 已完成并持久化最终结果。
    Completed,
    /// 被用户主动中断。
    Interrupted,
    /// 发生不可恢复错误。
    Failed,
}

/// 对外稳定的回合失败分类。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "message")]
pub enum TurnFailure {
    InvalidInput(String),
    Provider(String),
    Tool(String),
    Persistence(String),
    Cancelled(String),
}

#[cfg(test)]
mod tests {
    use super::{TurnFailure, TurnState};

    #[test]
    fn state_serialization_is_stable_and_readable() {
        assert_eq!(
            serde_json::to_string(&TurnState::AwaitingApproval).unwrap(),
            "\"awaiting_approval\""
        );
        let error = TurnFailure::Provider("超时".into());
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(json["kind"], "provider");
    }
}
