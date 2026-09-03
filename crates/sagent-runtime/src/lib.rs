//! Sagent 会话运行时。
//!
//! sagent-runtime 负责将一个 Session 的命令串行化，并由唯一的
//! SessionActor 调用 sagent-store 写入 Turn、消息和事件。
//! 外部调用者只能通过 SessionHandle 发送命令，不能直接访问 Store
//! 或修改 Turn 状态。
//!
//! 本 crate 当前只建立错误边界和公开 API 骨架；模型 HTTP 请求、工具执行
//! 和 TUI 渲染将在后续阶段实现。

mod error;

pub use error::RuntimeError;

/// 管理多个 SessionActor 的入口。
///
/// 实际的 SessionId 映射和 actor 生命周期将在后续步骤实现。
pub struct SessionSupervisor;

/// 一个会话 actor 的受限句柄。
///
/// 句柄只会投递命令，不暴露 Store、SQLite 连接或 actor 内部状态。
pub struct SessionHandle;
