//! sagent-runtime — Phase 1 Session Actor 基础运行时。
//!
//! 当前负责 Session Actor、Runtime Supervisor、启动恢复和有界订阅。
//! JSON-RPC 和 CLI 在后续 Step 实现。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 5 Session Actor

pub mod error;
pub mod event_bus;
pub mod recovery;
pub mod runtime;
pub mod session_actor;
pub mod session_command;
pub mod session_snapshot;
pub mod supervisor;

pub use error::{ActorError, RuntimeError};
pub use event_bus::{EventBus, EventReceiver, SessionEvent};
pub use runtime::Runtime;
pub use session_actor::{SessionActor, SessionHandle};
pub use session_command::{ActorCommand, CommandReply};
pub use session_snapshot::SessionSnapshot;
pub use supervisor::{RuntimeHandle, SessionView};
