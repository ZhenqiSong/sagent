//! Sagent 会话运行时。
//!
//! sagent-runtime 负责将一个 Session 的命令串行化，并由唯一的
//! SessionActor 调用 sagent-store 写入 Turn、消息和事件。
//! 外部调用者只能通过 SessionHandle 发送命令，不能直接访问 Store
//! 或修改 Turn 状态。
//!
//! 本 crate 当前已提供多会话 Supervisor（有界 mailbox 与 actor 生命周期）；
//! 模型 HTTP 请求、工具执行和 TUI 渲染将在后续阶段实现。

#[allow(dead_code)]
mod active_turn;
mod actor;
mod error;
mod event;
#[allow(dead_code)]
mod input;
mod supervisor;

#[cfg(test)]
mod test_support;

pub use error::RuntimeError;
pub use event::{RuntimeEvent, RuntimeEventKind, RuntimeEventSubscription, SubscriptionError};
pub use supervisor::{SessionHandle, SessionSupervisor, SubmitReceipt};
