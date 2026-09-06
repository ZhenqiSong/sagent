//! Sagent 跨 crate 共享的领域标识、会话、消息和 Turn 类型。
//!
//! 这些类型只描述数据和序列化约定，不包含数据库访问或运行时副作用。

pub mod capabilities;
pub mod error;
pub mod ids;
pub mod message;
pub mod session;
pub mod turn;

pub use capabilities::{ClientCapabilities, ClientSurface};
pub use ids::{ApprovalId, ClientId, MessageId, SessionId, ToolCallId, TurnId};
pub use message::{SearchHit, StoredMessage};
pub use session::{SessionDetail, SessionSummary};
pub use turn::{EventSequence, PersistedTurnStatus, TurnOutcome, TurnTypeError};
