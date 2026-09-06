//! SessionActor 的内部输入消息。

use crate::RuntimeError;
use crate::approval::{ApprovalOutcome, ApprovalRequest};
use crate::tool_call::ToolCall;
use crate::tool_worker::ToolExecutionResult;
use sagent_agent::SessionCommand;
use sagent_provider::TokenUsage;
use sagent_types::TurnId;
use tokio::sync::oneshot;

/// Actor 处理命令后返回给发送方的一次性结果。
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum CommandReply {
    /// prompt 已经完成 Store 原子提交。
    Accepted { turn_id: TurnId },
    /// 中断请求已经被 Actor 接收。
    Interrupted,
    /// 审批响应已经被 Actor 接收；工具是否执行由后续事件决定。
    ApprovalAccepted,
    /// 客户端能力握手已更新。
    Resumed,
    /// Actor 已经完成关闭。
    Closed,
}

/// Provider/tool worker 的受控失败信息。
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct WorkerFailure(pub(crate) String);

/// 投递给 SessionActor mailbox 的内部消息。
///
/// 外部调用者只能通过 SessionHandle 发送 SessionCommand，不能伪造
/// WorkerEvent 或直接取得 Actor 的 Store。
pub(crate) enum ActorInput {
    /// 来自 RPC、TUI 或 CLI 的领域命令。
    Command {
        command: SessionCommand,
        reply_to: oneshot::Sender<Result<CommandReply, RuntimeError>>,
    },
    /// 来自受 Actor 监管的 worker 的结果。
    Worker(WorkerEvent),
    /// worker task 结束后的生命周期通知。
    WorkerExited {
        turn_id: TurnId,
        result: Result<(), WorkerFailure>,
    },
    /// 工具 worker 发现当前调用需要用户审批。
    ApprovalRequired {
        turn_id: TurnId,
        request: ApprovalRequest,
    },
    /// 审批 waiter 将决定重新投递回 Actor，避免 Actor 阻塞在等待上。
    ApprovalOutcome {
        turn_id: TurnId,
        approval_id: sagent_types::ApprovalId,
        outcome: ApprovalOutcome,
    },
}

/// Provider/tool worker 回传给 Actor 的模型无关事件。
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum WorkerEvent {
    /// 一个流式文本片段；只用于临时 UI 展示。
    TextDelta { turn_id: TurnId, text: String },
    /// Provider 的 token 统计；只作为瞬态事件，不拼接到消息正文。
    Usage { turn_id: TurnId, usage: TokenUsage },
    /// 模型生成了最终文本。
    FinalText { turn_id: TurnId, text: String },
    /// worker 发生可控失败。
    Failed { turn_id: TurnId, reason: String },
    /// worker 响应取消令牌后停止。
    Cancelled { turn_id: TurnId },
    /// Provider 已完成一个包含工具调用的响应；工具由 Actor 统一分发。
    ToolCalls {
        turn_id: TurnId,
        text: String,
        calls: Vec<ToolCall>,
    },
    /// 工具 worker 完成当前调用后回传结果。类型保留 Vec 是为 worker 边界复用，
    /// Actor 目前强制它恰好包含一个结果，以便逐项审批和持久化。
    ToolResults {
        turn_id: TurnId,
        results: Vec<ToolExecutionResult>,
    },
}
