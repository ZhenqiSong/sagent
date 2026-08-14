//! sagent-session — SQLite 连接、版本化 schema 和 Session Repository。
//!
//! 本 crate 不依赖 JSON-RPC 或 Runtime，也不向业务层暴露裸 SQLite connection。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 3 SQLite 基础设施

pub mod connection;
pub mod error;
pub mod migrations;
pub mod models;
pub mod repository;

pub use connection::{DatabaseConnection, PragmaState};
pub use error::DatabaseError;
pub use migrations::{Migration, MIGRATIONS};
pub use models::{
    AppendMessage, CreateSession, ListSessions, MessageRange, SessionCursor, SessionSnapshot,
    SessionSummary, MAX_LIST_LIMIT, MAX_MESSAGE_LIMIT,
};
pub use repository::{Repository, RepositoryError};
