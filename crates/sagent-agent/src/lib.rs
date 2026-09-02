//! Sagent Agent 的纯领域层骨架。
//!
//! 4.1 首先只建立状态机、PromptSnapshot 和 transcript 不变量；本 crate 不依赖
//! SQLite、HTTP、Tokio 或终端 UI。后续 Runtime、Provider 和 TUI 都应通过这些类型
//! 协作，而不是各自复制会话状态判断。

pub mod command;
pub mod event;
pub mod prompt;
pub mod state;
pub mod transcript;
pub mod transition;

pub use command::{ApprovalDecision, CommandError, RequestId, SessionCommand, UserInput};
pub use event::TurnEvent;
pub use prompt::{PromptError, PromptMessage, PromptRole, PromptSnapshot, SystemPromptParts};
pub use state::{TurnFailure, TurnState};
pub use transcript::{Transcript, TranscriptError};
pub use transition::{TransitionError, apply_command};
