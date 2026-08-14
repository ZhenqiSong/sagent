//! SQLite connection 生命周期和 schema 初始化。
//!
//! 连接打开顺序固定为：路径/父目录、connection、PRAGMA、migration、schema 校验。
//! 任一步失败都返回错误，不允许 Runtime 使用半初始化数据库。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 3 SQLite connection

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;
use sagent_config::{ConfigPaths, DatabaseConfig, SynchronousMode};

use crate::error::DatabaseError;
use crate::migrations::{self, Migration, MIGRATIONS};

/// SQLite 实际生效的关键 PRAGMA 值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PragmaState {
    /// 外键约束是否开启。
    pub foreign_keys: bool,
    /// journal mode，默认应为 `wal`。
    pub journal_mode: String,
    /// busy timeout（毫秒）。
    pub busy_timeout_ms: u64,
    /// synchronous 数值：FULL=2、NORMAL=1、OFF=0。
    pub synchronous: u8,
}

/// 已完成初始化和 migration 的 SQLite 数据库连接。
pub struct DatabaseConnection {
    connection: Connection,
    path: PathBuf,
    schema_version: u32,
}

impl std::fmt::Debug for DatabaseConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DatabaseConnection")
            .field("path", &self.path)
            .field("schema_version", &self.schema_version)
            .finish_non_exhaustive()
    }
}

impl DatabaseConnection {
    /// 打开指定数据库，创建父目录并执行标准 migration。
    pub fn open(path: impl AsRef<Path>, config: &DatabaseConfig) -> Result<Self, DatabaseError> {
        Self::open_with_migrations(path, config, MIGRATIONS)
    }

    /// 按配置和 home 路径打开数据库；未指定路径时使用 `<home>/state.db`。
    pub fn open_from_config(
        paths: &ConfigPaths,
        config: &DatabaseConfig,
    ) -> Result<Self, DatabaseError> {
        let path = config.path.clone().unwrap_or_else(|| paths.root().join("state.db"));
        Self::open(path, config)
    }

    /// 打开指定数据库并执行传入的 migration；测试可用它注入失败 migration。
    pub fn open_with_migrations(
        path: impl AsRef<Path>,
        config: &DatabaseConfig,
        migrations_to_apply: &[Migration],
    ) -> Result<Self, DatabaseError> {
        config.validate().map_err(|error| DatabaseError::Config(error.to_string()))?;
        let path = path.as_ref().to_path_buf();
        create_parent(&path)?;
        let mut connection = Connection::open(&path).map_err(|source| DatabaseError::Open {
            path: path.clone(),
            source,
        })?;
        let pragma = configure_pragmas(&mut connection, config)?;
        let schema_version = migrations::apply(&mut connection, migrations_to_apply)?;
        validate_schema(&connection, schema_version, migrations_to_apply.last())?;
        if pragma.journal_mode != "wal" {
            return Err(DatabaseError::SchemaInvalid {
                object: format!("journal_mode={}", pragma.journal_mode),
            });
        }
        Ok(Self {
            connection,
            path,
            schema_version,
        })
    }

    /// 返回数据库文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 返回已应用的 schema 版本。
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// 读取当前连接的关键 PRAGMA 值。
    pub fn pragma_state(&self) -> Result<PragmaState, DatabaseError> {
        read_pragmas(&self.connection)
    }

    /// 查询指定表是否存在。
    pub fn table_exists(&self, name: &str) -> Result<bool, DatabaseError> {
        migrations::table_exists(&self.connection, name)
    }

    /// 查询指定索引是否存在。
    pub fn index_exists(&self, name: &str) -> Result<bool, DatabaseError> {
        migrations::index_exists(&self.connection, name)
    }

    /// 返回指定表的列名，用于 schema 自检和诊断。
    pub fn table_columns(&self, name: &str) -> Result<Vec<String>, DatabaseError> {
        migrations::table_columns(&self.connection, name)
    }

    pub(crate) fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    pub(crate) fn connection_ref(&self) -> &Connection {
        &self.connection
    }
}

fn create_parent(path: &Path) -> Result<(), DatabaseError> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|source| DatabaseError::CreateParent {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

fn configure_pragmas(
    connection: &mut Connection,
    config: &DatabaseConfig,
) -> Result<PragmaState, DatabaseError> {
    connection.busy_timeout(Duration::from_millis(config.busy_timeout_ms))?;
    connection.execute_batch("PRAGMA foreign_keys = ON")?;
    let journal_mode = connection.query_row("PRAGMA journal_mode = WAL", [], |row| {
        row.get::<_, String>(0)
    })?;
    let synchronous = match config.synchronous {
        SynchronousMode::Full => "FULL",
        SynchronousMode::Normal => "NORMAL",
        SynchronousMode::Off => "OFF",
    };
    connection.execute_batch(&format!("PRAGMA synchronous = {synchronous}"))?;
    let state = read_pragmas(connection)?;
    Ok(PragmaState {
        journal_mode: journal_mode.to_ascii_lowercase(),
        ..state
    })
}

fn read_pragmas(connection: &Connection) -> Result<PragmaState, DatabaseError> {
    let foreign_keys =
        connection.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))? == 1;
    let journal_mode =
        connection.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?;
    let busy_timeout_ms =
        connection.query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))? as u64;
    let synchronous =
        connection.query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))? as u8;
    Ok(PragmaState {
        foreign_keys,
        journal_mode: journal_mode.to_ascii_lowercase(),
        busy_timeout_ms,
        synchronous,
    })
}

fn validate_schema(
    connection: &Connection,
    version: u32,
    latest_migration: Option<&Migration>,
) -> Result<(), DatabaseError> {
    if Some(version) != latest_migration.map(|migration| migration.version) {
        return Err(DatabaseError::SchemaInvalid {
            object: format!("schema version {version}, expected latest migration"),
        });
    }
    for table in ["schema_meta", "sessions", "messages"] {
        if !migrations::table_exists(connection, table)? {
            return Err(DatabaseError::SchemaInvalid {
                object: format!("missing table {table}"),
            });
        }
    }
    for index in ["idx_sessions_updated_at", "idx_messages_session_sequence"] {
        if !migrations::index_exists(connection, index)? {
            return Err(DatabaseError::SchemaInvalid {
                object: format!("missing index {index}"),
            });
        }
    }
    let expected_sessions = [
        "id",
        "source",
        "title",
        "cwd",
        "status",
        "metadata_json",
        "created_at",
        "updated_at",
        "message_count",
        "revision",
    ];
    let expected_messages = [
        "id",
        "session_id",
        "sequence",
        "role",
        "content_json",
        "tool_calls_json",
        "tool_call_id",
        "metadata_json",
        "created_at",
    ];
    if migrations::table_columns(connection, "sessions")? != expected_sessions {
        return Err(DatabaseError::SchemaInvalid {
            object: "sessions columns".to_string(),
        });
    }
    if migrations::table_columns(connection, "messages")? != expected_messages {
        return Err(DatabaseError::SchemaInvalid {
            object: "messages columns".to_string(),
        });
    }
    Ok(())
}
