//! SQLite Session 业务存储适配器。
//!
//! 本模块只负责把一个 `SqliteDatabase` 资源组装成 `Storage`、`ReadStorage` 或
//! `WriteStorage` 所需的领域端口。端口对象共享同一受保护的数据库句柄，但不会把
//! SQLite 连接泄漏到 Runtime、RPC、CLI 或工具；后续替换为远程后端时，只需替换本模块。
//!
//! 写入、查询和搜索端口分别位于同目录子模块。业务操作的原子边界仍由
//! `SqliteDatabase` 的同主题实现保证，本模块只负责领域端口适配和最小权限装配。
//!
//! 作者：SongZQ

use std::sync::{Arc, Mutex, MutexGuard};

use crate::{
    ReadOnlySessionStorage, ReadStorage, SessionStorage, SqliteDatabase, Storage,
    StorageDependencies, StorageReadDependencies, StorageResult, WriteStorage,
};

mod query;
mod search;
mod write;

use query::SqliteSessionQueryStorage;
use search::SqliteSearchStorage;
use write::SqliteSessionStorage;

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

/// 将 SQLite 数据库句柄包装为兼容期的完整领域端口集合。
fn storage_dependencies_from_database(database: SqliteDatabase) -> StorageDependencies {
    let (write, query, search) = ports_from_database(database);
    StorageDependencies::new(write, query, search)
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

/// 兼容期 Factory 使用的 SQLite 端口转换，不向上层暴露连接细节。
impl From<SqliteDatabase> for StorageDependencies {
    fn from(database: SqliteDatabase) -> Self {
        storage_dependencies_from_database(database)
    }
}

/// 只读 SQLite 句柄只能转换为查询和搜索端口，不能获得写入能力。
impl From<SqliteDatabase> for StorageReadDependencies {
    fn from(database: SqliteDatabase) -> Self {
        let shared = Arc::new(Mutex::new(database));
        StorageReadDependencies::new(
            SqliteSessionQueryStorage {
                database: Arc::clone(&shared),
            },
            SqliteSearchStorage { database: shared },
        )
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
