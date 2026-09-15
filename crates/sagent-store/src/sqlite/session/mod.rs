//! SQLite Session 领域持久化实现。
//!
//! 本模块同时收纳 Session 领域的 SQL 映射和端口适配：查询、搜索、Turn、Event 以及
//! 跨表事务各自位于独立子模块。`storage_from_database` 只负责把这些实现组装成抽象
//! `Storage`，不会把 SQLite 连接泄漏到 Runtime、RPC、CLI 或工具。
//!
//! 业务操作的原子边界仍由 `transactions` 中的高层操作保证；`SqliteDatabase` 只提供
//! 受访问模式保护的连接资源。后续增加其他业务领域时，应在 `sqlite` 下新增领域包，
//! 不要把无关 SQL 追加到本模块。
//!
//! 作者：SongZQ

use std::sync::{Arc, Mutex, MutexGuard};

use super::database::SqliteDatabase;
use crate::{
    ReadOnlySessionStorage, ReadStorage, SessionStorage, Storage, StorageResult, WriteStorage,
};

mod events;
mod messages;
mod queries;
mod read_port;
mod search;
mod search_port;
mod transactions;
mod turns;
mod write_port;

use read_port::SqliteSessionQueryStorage;
use search_port::SqliteSearchStorage;
use write_port::SqliteSessionStorage;

pub use events::{
    EVENT_APPROVAL_REQUESTED, EVENT_APPROVAL_RESOLVED, EVENT_APPROVAL_TIMED_OUT,
    EVENT_MESSAGE_COMMITTED, EVENT_TOOL_COMPLETED, EVENT_TOOL_STARTED, EVENT_TURN_COMPLETED,
    EVENT_TURN_FAILED, EVENT_TURN_INTERRUPTED, EVENT_TURN_STARTED, EventQuery, MAX_EVENT_LIMIT,
    NewDaemonEvent, StoredDaemonEvent,
};
pub use messages::{MessageQuery, MessageWindow};
pub use queries::SessionListQuery;
pub use search::MessageSearchQuery;
pub use transactions::{
    NewMessage, NewSession, RestoreResult, RetryCheckpoint, RewindCheckpoint, RewindResult,
};
pub use turns::{NewGeneration, StartTurn, StoredGeneration, StoredRunningTurn};

type SharedDatabase = Arc<Mutex<SqliteDatabase>>;

/// 使用一个 SQLite 数据库句柄组装完整的 Session 业务存储。
pub(crate) fn storage_from_database(database: SqliteDatabase) -> Storage {
    let (write, query, search) = ports_from_database(database);
    Storage::new(SessionStorage::new(
        write,
        ReadOnlySessionStorage::new(query, search),
    ))
}

/// 使用一个 SQLite 数据库句柄组装只读业务存储。
pub(crate) fn read_storage_from_database(database: SqliteDatabase) -> ReadStorage {
    let shared = Arc::new(Mutex::new(database));
    ReadStorage::new(ReadOnlySessionStorage::new(
        SqliteSessionQueryStorage {
            database: Arc::clone(&shared),
        },
        SqliteSearchStorage { database: shared },
    ))
}

/// 使用一个 SQLite 数据库句柄组装最小写入业务存储。
pub(crate) fn write_storage_from_database(database: SqliteDatabase) -> WriteStorage {
    let shared = Arc::new(Mutex::new(database));
    WriteStorage::new(crate::WriteOnlySessionStorage::new(SqliteSessionStorage {
        database: shared,
    }))
}

/// 为完整 Actor 存储一次性装配共享数据库句柄，保证写入、查询和搜索看到同一资源。
fn ports_from_database(
    database: SqliteDatabase,
) -> (
    SqliteSessionStorage,
    SqliteSessionQueryStorage,
    SqliteSearchStorage,
) {
    let shared = Arc::new(Mutex::new(database));
    (
        SqliteSessionStorage {
            database: Arc::clone(&shared),
        },
        SqliteSessionQueryStorage {
            database: Arc::clone(&shared),
        },
        SqliteSearchStorage { database: shared },
    )
}

/// 将 SQLite 数据库句柄转换为完整业务存储，供测试和 adapter 装配边界使用。
impl From<SqliteDatabase> for Storage {
    fn from(database: SqliteDatabase) -> Self {
        storage_from_database(database)
    }
}

/// 只读 SQLite 句柄只能转换为查询和搜索端口，不能获得写入能力。
impl From<SqliteDatabase> for ReadStorage {
    fn from(database: SqliteDatabase) -> Self {
        read_storage_from_database(database)
    }
}

/// 将 Mutex 中毒转换成不泄漏实现细节的存储错误。
pub(super) fn lock_database(
    database: &SharedDatabase,
) -> StorageResult<MutexGuard<'_, SqliteDatabase>> {
    database
        .lock()
        .map_err(|_| anyhow::anyhow!("SQLite 存储锁已中毒"))
}
