//! Sagent 会话运行时。
//!
//! sagent-runtime 负责将一个 Session 的命令串行化，并由唯一的
//! SessionActor 调用 sagent-store 写入 Turn、消息和事件。
//! 外部调用者只能通过 SessionHandle 发送命令，不能直接访问 Store
//! 或修改 Turn 状态。
//!
//! 本 crate 还负责监管 Provider 流、工具/审批回环、CancellationToken 传播与
//! 重启后的 fail-closed 恢复；TUI/RPC 仅通过事件订阅与 SessionHandle 接入。

#[allow(dead_code)]
mod active_turn;
mod actor;
mod approval;
mod error;
mod event;
#[allow(dead_code)]
mod input;
mod provider_worker;
mod recovery;
mod supervisor;
mod tool_call;
mod tool_dispatch;
mod tool_worker;

#[cfg(test)]
mod test_support;

pub use approval::{
    ApprovalError, ApprovalManager, ApprovalOutcome, ApprovalRequest, ApprovalWaiter,
};
pub use error::RuntimeError;
pub use event::{RuntimeEvent, RuntimeEventKind, RuntimeEventSubscription, SubscriptionError};
pub use supervisor::{SessionHandle, SessionSupervisor, SubmitReceipt};
pub use tool_call::{ToolCall, ToolCallAccumulator, ToolCallError};
pub use tool_dispatch::{ToolDispatchError, ToolDispatchPlan, ToolDispatcher};
pub use tool_worker::{ToolExecutionResult, ToolWorker, ToolWorkerError};
