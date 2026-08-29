//! Sagent SQLite 存储访问层。
//!
//! 作者：SongZQ
//! 创建日期：2026-08-29
//! 变更记录：
//! - 2026-08-29：实现只读 SQLite 连接与基础连接测试。

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

/// Sagent 持久化存储的只读访问入口。
#[derive(Debug)]
pub struct Store {
    connection: Connection,
}

impl Store {
    /// 以只读方式打开已有的 state.db。
    ///
    /// 此方法不会创建数据库、执行 migration、修改 journal mode，
    /// 也不会修复 FTS 或写入任何诊断数据。
    pub fn open_readonly(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            anyhow::bail!("state.db 路径必须是绝对路径");
        }

        if !path.is_file() {
            anyhow::bail!("state.db 不存在或不是普通文件：{}", path.display());
        }

        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        )
        .with_context(|| format!("无法以只读方式打开 state.db：{}", path.display()))?;

        Ok(Self { connection })
    }

    /// 验证连接仍然可执行基本查询。
    pub fn verify_connection(&self) -> Result<()> {
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
        std::env::temp_dir().join(format!("sagent-store-{name}-{}.db", std::process::id()))
    }

    fn remove_if_exists(path: &std::path::Path) {
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_relative_database_path() {
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
        assert!(!path.exists(), "只读打开不能创建数据库文件");
    }

    #[test]
    fn opens_existing_database_in_readonly_mode() {
        let path = test_path("readonly");
        remove_if_exists(&path);

        {
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

        let write_result = store
            .connection
            .execute("INSERT INTO marker (value) VALUES (8)", []);
        assert!(write_result.is_err(), "只读连接不应允许写入");

        remove_if_exists(&path);
    }
}
