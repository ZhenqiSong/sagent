//! sagent-runtime — Phase 1 Session Actor 基础运行时。
//!
//! 当前只负责单个 Session 的串行命令处理、持久化后事件和有界订阅。
//! Supervisor、JSON-RPC 和 CLI 在后续 Step 实现。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 5 Session Actor

pub mod error;
pub mod event_bus;
pub mod session_actor;
pub mod session_command;
pub mod session_snapshot;

pub use error::ActorError;
pub use event_bus::{EventBus, EventReceiver, SessionEvent};
pub use session_actor::{SessionActor, SessionHandle};
pub use session_command::{ActorCommand, CommandReply};
pub use session_snapshot::SessionSnapshot;
