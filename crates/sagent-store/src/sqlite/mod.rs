//! SQLite 后端实现的模块地图。
//!
//! `sqlite` 统一收纳连接生命周期、schema/migration、Manager 以及按领域划分的
//! Session 持久化实现。上层只通过 `sagent_store` 根模块导出的抽象 Storage 和兼容期
//! 类型访问本包；连接、SQL 和表模型不跨出这个后端边界。
//!
//! `session` 子模块承载当前会话领域的查询、搜索、Turn、Event 与事务操作。后续增加
//! Profile、调度等持久化领域时，应在本包内增加同级业务子模块，而不是把 SQL 追加到
//! `database.rs` 或 Manager 中。
//!
//! 作者：SongZQ

mod database;
mod manager;
mod migration;
mod schema;
mod session;

pub use database::SqliteDatabase;
pub use manager::SqliteStorageManager;

pub use migration::SCHEMA_VERSION;
pub use schema::DatabaseInfo;
pub use session::{
    EVENT_APPROVAL_REQUESTED, EVENT_APPROVAL_RESOLVED, EVENT_APPROVAL_TIMED_OUT,
    EVENT_MESSAGE_COMMITTED, EVENT_TOOL_COMPLETED, EVENT_TOOL_STARTED, EVENT_TURN_COMPLETED,
    EVENT_TURN_FAILED, EVENT_TURN_INTERRUPTED, EVENT_TURN_STARTED, EventQuery, MAX_EVENT_LIMIT,
    MessageQuery, MessageSearchQuery, MessageWindow, NewDaemonEvent, NewGeneration, NewMessage,
    NewSession, RestoreResult, RetryCheckpoint, RewindCheckpoint, RewindResult, SessionListQuery,
    StartTurn, StoredDaemonEvent, StoredGeneration, StoredRunningTurn,
};
