//! Sagent 持久化领域端口与 SQLite 存储实现。
//!
//! 端口模块只表达业务持久化行为；`SqliteDatabase`、`sqlite_factory` 和 `sqlite_manager` 封装
//! SQLite 连接、事务与 migration。上层 Runtime、RPC、CLI 和工具应通过
//! `StorageManager` 按职责获取端口，`StorageFactory` 只停留在 bootstrap/selector 构造
//! 边界，避免把具体数据库路径或连接生命周期扩散到业务编排代码。
//!
//! 作者：SongZQ
//! 创建日期：2026-08-29

mod event;
mod message;
mod migration;
pub mod ports;
mod schema;
mod search;
mod session;
mod sqlite_database;
pub mod sqlite_factory;
pub mod sqlite_manager;
mod sqlite_session_storage;
mod turn;
mod write;

pub use event::{
    EVENT_APPROVAL_REQUESTED, EVENT_APPROVAL_RESOLVED, EVENT_APPROVAL_TIMED_OUT,
    EVENT_MESSAGE_COMMITTED, EVENT_TOOL_COMPLETED, EVENT_TOOL_STARTED, EVENT_TURN_COMPLETED,
    EVENT_TURN_FAILED, EVENT_TURN_INTERRUPTED, EVENT_TURN_STARTED, EventQuery, MAX_EVENT_LIMIT,
    NewDaemonEvent, StoredDaemonEvent,
};
pub use message::{MessageQuery, MessageWindow};
pub use migration::SCHEMA_VERSION;
pub use ports::{
    ReadOnlySessionStorage, ReadStorage, SearchStorage, SessionQueryStorage, SessionStorage,
    SessionWriteStorage, Storage, StorageDependencies, StorageFactory, StorageManager,
    StorageReadDependencies, StorageResult, StorageWriteDependencies, WriteOnlySessionStorage,
    WriteStorage,
};
pub use schema::DatabaseInfo;
pub use search::MessageSearchQuery;
pub use session::SessionListQuery;
pub use sqlite_database::SqliteDatabase;
pub use sqlite_factory::SqliteStorageFactory;
pub use sqlite_manager::SqliteStorageManager;
pub use turn::{NewGeneration, StartTurn, StoredGeneration, StoredRunningTurn};
pub use write::{
    NewMessage, NewSession, RestoreResult, RetryCheckpoint, RewindCheckpoint, RewindResult,
};

// SQLite 数据库的黑盒/事务契约测试与入口实现分离，防止连接打开规则被数百行 fixture 淹没；
// 测试仍作为 crate 子模块保留，因此可以验证只读边界而无需向生产 API 暴露连接。
#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
