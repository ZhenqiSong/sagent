//! Sagent Agent 的纯领域层骨架。
//!
//! 本 crate 提供状态机、PromptSnapshot 和 transcript 不变量；不依赖 SQLite、HTTP、
//! Tokio 或终端 UI。Runtime、Provider 和 TUI 都应通过这些类型协作，而不是各自复制
//! 会话状态判断。

pub mod command;
pub mod event;
pub mod prompt;
pub mod state;
pub mod transcript;
pub mod transition;

pub use command::{ApprovalDecision, CommandError, RequestId, SessionCommand, UserInput};
pub use event::TurnEvent;
pub use prompt::{
    PromptError, PromptMessage, PromptRole, PromptSnapshot, PromptToolCall, SystemPromptParts,
};
pub use state::{TurnFailure, TurnState};
pub use transcript::{Transcript, TranscriptError};
pub use transition::{TransitionError, apply_command, apply_event};
