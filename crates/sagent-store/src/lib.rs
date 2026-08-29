//! Sagent SQLite 存储访问层。
//!
//! 作者：SongZQ
//! 创建日期：2026-08-29

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

pub mod schema;
pub mod session;

pub use schema::DatabaseInfo;

/// Sagent 持久化存储的只读访问入口。
#[derive(Debug)]
pub struct Store {
    // Connection 保持私有，避免上层绕过 Store 的只读约束直接执行任意 SQL。
    connection: Connection,
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

        Ok(Self { connection })
    }

    /// 验证连接仍然可执行基本查询。
    pub fn verify_connection(&self) -> Result<()> {
        // 使用无副作用的常量查询验证连接，而不是读取某张尚未确认存在的业务表。
        self.connection
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .context("state.db 基本查询失败")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use rusqlite::Connection;

    use super::Store;

    fn test_path(name: &str) -> PathBuf {
        // 每个测试使用独立文件名，避免并行测试共享数据库；文件位于系统临时目录，
        // 不会触碰开发者真实的 SAGENT_HOME。
        std::env::temp_dir().join(format!("sagent-store-{name}-{}.db", std::process::id()))
    }

    fn remove_if_exists(path: &std::path::Path) {
        // 清理函数允许目标不存在，便于在测试开始前消除上次异常留下的临时文件。
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_relative_database_path() {
        // 相对路径会随当前工作目录变化，不能作为持久化数据库的安全边界。
        let result = Store::open_readonly(std::path::Path::new("state.db"));

        assert!(result.is_err());
        let error = result.expect_err("相对路径必须失败");
        assert!(error.to_string().contains("绝对路径"));
    }

    #[test]
    fn rejects_missing_database_without_creating_it() {
        let path = test_path("missing");
        remove_if_exists(&path);

        let result = Store::open_readonly(&path);

        assert!(result.is_err());
        // 这是只读打开最重要的副作用契约：缺失文件不能被自动初始化。
        assert!(!path.exists(), "只读打开不能创建数据库文件");
    }

    #[test]
    fn opens_existing_database_in_readonly_mode() {
        let path = test_path("readonly");
        remove_if_exists(&path);

        {
            // 仅测试准备阶段使用可写连接创建 fixture；被测 Store 始终使用只读连接。
            let connection = Connection::open(&path).expect("应能创建测试数据库");
            connection
                .execute("CREATE TABLE marker (value INTEGER NOT NULL)", [])
                .expect("应能创建测试表");
            connection
                .execute("INSERT INTO marker (value) VALUES (7)", [])
                .expect("应能写入测试数据");
        }

        let store = Store::open_readonly(&path).expect("已有数据库应能只读打开");
        store.verify_connection().expect("基本查询应成功");

        // 直接通过私有字段验证 SQLite 层面的写保护，而不仅仅是验证 SELECT 成功。
        let write_result = store
            .connection
            .execute("INSERT INTO marker (value) VALUES (8)", []);
        assert!(write_result.is_err(), "只读连接不应允许写入");

        // 测试结束后删除 fixture，避免临时目录累积数据库文件。
        remove_if_exists(&path);
    }
}
