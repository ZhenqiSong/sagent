//! Sagent 会话运行时。
//!
//! sagent-runtime 负责将一个 Session 的命令串行化，并由唯一的
//! SessionActor 调用 sagent-store 写入 Turn、消息和事件。
//! 外部调用者只能通过 SessionHandle 发送命令，不能直接访问 Store
//! 或修改 Turn 状态。
//!
//! 本 crate 还负责监管 Provider 流、工具/审批回环、CancellationToken 传播与
//! 重启后的 fail-closed 恢复；TUI/RPC 仅通过事件订阅与 SessionHandle 接入。

// Actor 的状态、输入与重启恢复共同定义 mailbox 生命周期；物理目录集中它们，
// 但保持 crate 内模块名不变，避免把纯目录整理变成调用方的行为变更。
#[path = "actor_support/active_turn.rs"]
#[allow(dead_code)]
mod active_turn;
mod actor;
mod approval;
mod error;
mod event;
#[path = "actor_support/input.rs"]
#[allow(dead_code)]
mod input;
#[path = "worker/provider.rs"]
mod provider_worker;
#[path = "actor_support/recovery.rs"]
mod recovery;
mod supervisor;
#[path = "tooling/call.rs"]
mod tool_call;
#[path = "tooling/dispatch.rs"]
mod tool_dispatch;
#[path = "worker/tool.rs"]
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
