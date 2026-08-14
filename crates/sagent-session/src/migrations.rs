//! SQLite schema migration 定义和执行。
//!
//! 每个 migration 具有不可变、单调递增版本；版本更新和 SQL 执行在同一个事务中完成。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 3 migration runner

use rusqlite::{Connection, TransactionBehavior};

use crate::error::DatabaseError;

/// 一条不可变 SQLite migration。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Migration {
    /// 单调递增版本号。
    pub version: u32,
    /// 稳定名称。
    pub name: &'static str,
    /// migration SQL。
    pub sql: &'static str,
}

/// Sagent Phase 1 schema migration 列表。
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "0001_initial",
        sql: include_str!("../../../migrations/sqlite/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "0002_indexes",
        sql: include_str!("../../../migrations/sqlite/0002_indexes.sql"),
    },
];

/// 执行全部待处理 migration，并校验最终版本。
pub(crate) fn apply(
    connection: &mut Connection,
    migrations: &[Migration],
) -> Result<u32, DatabaseError> {
    validate_migration_order(migrations)?;
    let current = read_current_version(connection)?;
    let latest = migrations.last().map_or(0, |migration| migration.version);
    if current > latest {
        return Err(DatabaseError::Unsupported {
            reason: format!("数据库版本 {current} 高于当前支持版本 {latest}"),
        });
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut version = current;
    for migration in migrations.iter().filter(|migration| migration.version > current) {
        transaction
            .execute_batch(migration.sql)
            .map_err(|source| DatabaseError::Migration {
                version: migration.version,
                name: migration.name,
                source,
            })?;
        transaction
            .execute(
                "INSERT INTO schema_meta(key, value) VALUES('current_version', ?1) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [migration.version.to_string()],
            )
            .map_err(|source| DatabaseError::Migration {
                version: migration.version,
                name: migration.name,
                source,
            })?;
        version = migration.version;
    }
    transaction.commit()?;
    Ok(version)
}

fn validate_migration_order(migrations: &[Migration]) -> Result<(), DatabaseError> {
    if migrations.windows(2).any(|window| window[0].version >= window[1].version) {
        return Err(DatabaseError::Unsupported {
            reason: "migration 版本必须严格递增".to_string(),
        });
    }
    Ok(())
}

fn read_current_version(connection: &Connection) -> Result<u32, DatabaseError> {
    let schema_meta_exists = table_exists(connection, "schema_meta")?;
    let has_user_tables = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master \
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%')",
        [],
        |row| row.get::<_, i64>(0),
    )? == 1;

    if !schema_meta_exists {
        if has_user_tables {
            return Err(DatabaseError::Unsupported {
                reason: "数据库存在表但缺少 Sagent schema_meta".to_string(),
            });
        }
        return Ok(0);
    }

    let columns = table_columns(connection, "schema_meta")?;
    if columns != ["key", "value"] {
        return Err(DatabaseError::Unsupported {
            reason: "schema_meta 结构不属于 Sagent schema".to_string(),
        });
    }

    let value = connection
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'current_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => DatabaseError::Unsupported {
                reason: "schema_meta 缺少 current_version".to_string(),
            },
            other => DatabaseError::Sqlite(other),
        })?;
    value.parse::<u32>().map_err(|_| DatabaseError::Unsupported {
        reason: "schema_meta.current_version 不是合法整数".to_string(),
    })
}

pub(crate) fn table_exists(connection: &Connection, name: &str) -> Result<bool, DatabaseError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |row| row.get::<_, i64>(0),
    )? == 1)
}

pub(crate) fn index_exists(connection: &Connection, name: &str) -> Result<bool, DatabaseError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
        [name],
        |row| row.get::<_, i64>(0),
    )? == 1)
}

pub(crate) fn table_columns(
    connection: &Connection,
    name: &str,
) -> Result<Vec<String>, DatabaseError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({name})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns)
}
