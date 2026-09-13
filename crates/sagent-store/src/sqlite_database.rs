//! SQLite 数据库连接与生命周期边界。
//!
//! 本模块只负责打开连接、设置 SQLite 的访问模式、执行 migration 和检查连接健康度。
//! 会话、消息、Turn 等业务操作仍由相邻的领域实现模块提供；这样后续替换为远程数据库
//! 时，业务端口不需要感知连接对象的具体类型。
//!
//! 作者：SongZQ

use std::{fs, path::Path};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

/// Sagent SQLite 后端的基础数据库句柄。
///
/// 句柄封装连接和读写能力，阻止调用方绕过访问模式直接执行 SQL。业务方法通过其他
/// 模块的 `impl SqliteDatabase` 提供；这些方法共享同一事务和只读保护边界。
#[derive(Debug)]
pub struct SqliteDatabase {
    // Connection 保持私有，避免上层绕过数据库句柄的只读约束直接执行任意 SQL。
    pub(crate) connection: Connection,
    pub(crate) writable: bool,
}

impl SqliteDatabase {
    /// 以只读方式打开已有的 state.db。
    ///
    /// 此方法不会创建数据库、执行 migration、修改 journal mode，也不会修复 FTS
    /// 或写入任何诊断数据。
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
            // READ_ONLY：禁止写入；NO_MUTEX：连接只在当前数据库句柄线程使用；
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
    /// 因为后者只能通过 `open_readonly` 兼容读取。
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
        crate::migration::migrate(&mut connection)?;
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

    /// 阻止只读数据库句柄被误用于写接口，即使调用方持有可变引用。
    pub(crate) fn ensure_writable(&self) -> Result<()> {
        if !self.writable {
            anyhow::bail!("当前 SQLite 数据库以只读模式打开，不能执行写操作");
        }
        Ok(())
    }
}
