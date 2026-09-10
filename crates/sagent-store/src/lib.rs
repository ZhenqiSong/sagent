//! Sagent SQLite 存储访问层。
//!
//! 作者：SongZQ
//! 创建日期：2026-08-29

use std::{fs, path::Path};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

pub mod event;
pub mod message;
pub mod migration;
pub mod schema;
pub mod search;
pub mod session;
pub mod turn;
pub mod write;

pub use event::{
    EVENT_APPROVAL_REQUESTED, EVENT_APPROVAL_RESOLVED, EVENT_APPROVAL_TIMED_OUT,
    EVENT_MESSAGE_COMMITTED, EVENT_TOOL_COMPLETED, EVENT_TOOL_STARTED, EVENT_TURN_COMPLETED,
    EVENT_TURN_FAILED, EVENT_TURN_INTERRUPTED, EVENT_TURN_STARTED, EventQuery, MAX_EVENT_LIMIT,
    NewDaemonEvent, StoredDaemonEvent,
};
pub use message::{MessageQuery, MessageWindow};
pub use migration::SCHEMA_VERSION;
pub use schema::DatabaseInfo;
pub use search::MessageSearchQuery;
pub use session::SessionListQuery;
pub use turn::{NewGeneration, StartTurn, StoredGeneration, StoredRunningTurn};
pub use write::{
    NewMessage, NewSession, RestoreResult, RetryCheckpoint, RewindCheckpoint, RewindResult,
};

// Store 的黑盒/事务契约测试与入口实现分离，防止连接打开规则被数百行 fixture 淹没；
// 测试仍作为 crate 子模块保留，因此可以验证只读边界而无需向生产 API 暴露连接。
#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;

/// Sagent 持久化存储的只读访问入口。
#[derive(Debug)]
pub struct Store {
    // Connection 保持私有，避免上层绕过 Store 的只读约束直接执行任意 SQL。
    connection: Connection,
    writable: bool,
}

impl Store {
    /// 以只读方式打开已有的 state.db。
    ///
    /// 此方法不会创建数据库、执行 migration、修改 journal mode，
    /// 也不会修复 FTS 或写入任何诊断数据。
    pub fn open_readonly(path: &Path) -> Result<Self> {
        // 先检查路径，再交给 SQLite，确保错误信息明确且不会因为 SQLite 的默认行为
        // 意外创建新数据库。
        if !path.is_absolute() {
            anyhow::bail!("state.db 路径必须是绝对路径");
        }

        if !path.is_file() {
            anyhow::bail!("state.db 不存在或不是普通文件：{}", path.display());
        }

        let connection = Connection::open_with_flags(
            path,
            // READ_ONLY：禁止写入；NO_MUTEX：连接只在当前 Store 所在线程使用；
            // URI：让 SQLite 使用标准 URI/只读打开语义。
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        )
        .with_context(|| format!("无法以只读方式打开 state.db：{}", path.display()))?;

        Ok(Self {
            connection,
            writable: false,
        })
    }

    /// 打开或创建 Sagent 自有数据库，并在事务中迁移至当前结构版本。
    ///
    /// 该入口只用于 Sagent 管理的数据库文件；不要将 Hermes 的 state.db 交给它，
    /// 因为后者只能通过 open_readonly 兼容读取。
    pub fn open_readwrite(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            anyhow::bail!("state.db 路径必须是绝对路径");
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建 state.db 父目录失败：{}", parent.display()))?;
        }
        let mut connection = Connection::open(path)
            .with_context(|| format!("无法以读写方式打开 state.db：{}", path.display()))?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .context("启用 SQLite 外键约束失败")?;
        migration::migrate(&mut connection)?;
        Ok(Self {
            connection,
            writable: true,
        })
    }

    /// 验证连接仍然可执行基本查询。
    pub fn verify_connection(&self) -> Result<()> {
        // 使用无副作用的常量查询验证连接，而不是读取某张尚未确认存在的业务表。
        self.connection
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .context("state.db 基本查询失败")?;
        Ok(())
    }

    /// 阻止只读 Store 被误用于写接口，即使调用方持有可变引用。
    fn ensure_writable(&self) -> Result<()> {
        if !self.writable {
            anyhow::bail!("当前 Store 以只读模式打开，不能执行写操作");
        }
        Ok(())
    }
}
