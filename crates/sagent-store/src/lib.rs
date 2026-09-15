//! Sagent 持久化领域端口与 SQLite 存储实现。
//!
//! 端口模块只表达业务持久化行为；`sqlite` 包统一封装 SQLite 连接、事务、migration
//! 和按领域划分的适配器。上层 Runtime、RPC、CLI 和工具应通过 `StorageManager` 按职责
//! 获取业务存储，避免把具体数据库路径或连接生命周期扩散到业务编排代码。
//!
//! 作者：SongZQ
//! 创建日期：2026-08-29

pub mod ports;
mod selector;
mod sqlite;

pub use ports::{
    ReadOnlySessionStorage, ReadStorage, SearchStorage, SessionQueryStorage, SessionStorage,
    SessionWriteStorage, Storage, StorageManager, StorageResult, WriteOnlySessionStorage,
    WriteStorage,
};
pub use selector::create_storage_manager;
pub use sqlite::{
    DatabaseInfo, MessageQuery, MessageSearchQuery, MessageWindow, NewGeneration, NewMessage,
    NewSession, RestoreResult, RetryCheckpoint, RewindCheckpoint, RewindResult, SCHEMA_VERSION,
    SessionListQuery, SqliteDatabase, SqliteStorageManager, StartTurn, StoredGeneration,
    StoredRunningTurn,
};
pub use sqlite::{
    EVENT_APPROVAL_REQUESTED, EVENT_APPROVAL_RESOLVED, EVENT_APPROVAL_TIMED_OUT,
    EVENT_MESSAGE_COMMITTED, EVENT_TOOL_COMPLETED, EVENT_TOOL_STARTED, EVENT_TURN_COMPLETED,
    EVENT_TURN_FAILED, EVENT_TURN_INTERRUPTED, EVENT_TURN_STARTED, EventQuery, MAX_EVENT_LIMIT,
    NewDaemonEvent, StoredDaemonEvent,
};

// SQLite 数据库的黑盒/事务契约测试与入口实现分离，防止连接打开规则被数百行 fixture 淹没；
// 测试仍作为 crate 子模块保留，因此可以验证只读边界而无需向生产 API 暴露连接。
#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
