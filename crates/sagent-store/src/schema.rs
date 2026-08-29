//! SQLite 数据库结构探测。
//!
//! 本模块只负责读取数据库元数据，不执行 migration，也不改变数据库内容。
//! 作者：SongZQ

use anyhow::{Context, Result};
use rusqlite::OptionalExtension;

use crate::Store;

/// 数据库结构的只读快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseInfo {
    /// `schema_version.version` 的第一条记录，没有标准版本表时为 `None`。
    pub schema_version: Option<i64>,
    /// 用户表名称，按字典序排列；SQLite 内部表会被排除。
    pub tables: Vec<String>,
    /// 是否检测到通过 `USING fts5` 创建的虚拟表。
    pub has_fts5: bool,
}

impl Store {
    /// 读取当前数据库的结构摘要。
    pub fn inspect_schema(&self) -> Result<DatabaseInfo> {
        // 所有探测都是 SELECT/PRAGMA，不会触发初始化或修改数据库。
        Ok(DatabaseInfo {
            schema_version: self.read_schema_version()?,
            tables: self.list_tables()?,
            has_fts5: self.detect_fts5()?,
        })
    }

    /// 读取非 SQLite 内部表名称。
    pub fn list_tables(&self) -> Result<Vec<String>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .context("读取数据库表列表失败")?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .context("查询数据库表列表失败")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("解析数据库表列表失败")
    }

    /// 读取可选的 `schema_version` 表，兼容没有 `version` 列的旧数据库。
    fn read_schema_version(&self) -> Result<Option<i64>> {
        let mut statement = self
            .connection
            .prepare("PRAGMA table_info(schema_version)")
            .context("读取 schema_version 表结构失败")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .context("查询 schema_version 列失败")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("解析 schema_version 列失败")?;
        if !columns.iter().any(|column| column == "version") {
            return Ok(None);
        }
        self.connection
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .optional()
            .context("读取 schema_version.version 失败")
    }

    /// 通过 SQLite 保存的建表 SQL 检测 FTS5，而不是依赖表名约定。
    fn detect_fts5(&self) -> Result<bool> {
        let count: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND sql IS NOT NULL
                   AND lower(sql) LIKE '%using fts5%'",
                [],
                |row| row.get(0),
            )
            .context("检测 FTS5 表失败")?;
        Ok(count > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::Store;
    use rusqlite::Connection;
    use std::{fs, path::PathBuf};

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sagent-schema-{name}-{}.db", std::process::id()))
    }
    fn remove(path: &std::path::Path) {
        let _ = fs::remove_file(path);
    }

    #[test]
    fn empty_database_has_no_schema_metadata() {
        let path = test_path("empty");
        remove(&path);
        Connection::open(&path).unwrap();
        let info = Store::open_readonly(&path)
            .unwrap()
            .inspect_schema()
            .unwrap();
        assert_eq!(info.schema_version, None);
        assert!(info.tables.is_empty());
        assert!(!info.has_fts5);
        remove(&path);
    }

    #[test]
    fn reads_schema_version_and_user_tables() {
        let path = test_path("version");
        remove(&path);
        let c = Connection::open(&path).unwrap();
        c.execute("CREATE TABLE schema_version (version INTEGER NOT NULL)", [])
            .unwrap();
        c.execute("INSERT INTO schema_version(version) VALUES (7)", [])
            .unwrap();
        c.execute("CREATE TABLE sessions (id TEXT PRIMARY KEY)", [])
            .unwrap();
        drop(c);
        let info = Store::open_readonly(&path)
            .unwrap()
            .inspect_schema()
            .unwrap();
        assert_eq!(info.schema_version, Some(7));
        assert_eq!(info.tables, vec!["schema_version", "sessions"]);
        assert!(!info.has_fts5);
        remove(&path);
    }

    #[test]
    fn detects_fts5_by_create_statement() {
        let path = test_path("fts5");
        remove(&path);
        let c = Connection::open(&path).unwrap();
        c.execute("CREATE VIRTUAL TABLE messages_fts USING fts5(content)", [])
            .unwrap();
        drop(c);
        let info = Store::open_readonly(&path)
            .unwrap()
            .inspect_schema()
            .unwrap();
        assert!(info.has_fts5);
        assert!(info.tables.iter().any(|t| t == "messages_fts"));
        remove(&path);
    }

    #[test]
    fn tolerates_non_standard_version_table() {
        let path = test_path("external");
        remove(&path);
        let c = Connection::open(&path).unwrap();
        c.execute("CREATE TABLE schema_version (value TEXT)", [])
            .unwrap();
        c.execute("CREATE TABLE unrelated (value TEXT)", [])
            .unwrap();
        drop(c);
        let info = Store::open_readonly(&path)
            .unwrap()
            .inspect_schema()
            .unwrap();
        assert_eq!(info.schema_version, None);
        assert_eq!(info.tables, vec!["schema_version", "unrelated"]);
        remove(&path);
    }

    #[test]
    fn inspection_does_not_write_to_readonly_database() {
        let path = test_path("readonly");
        remove(&path);
        let c = Connection::open(&path).unwrap();
        c.execute("CREATE TABLE marker (value INTEGER)", [])
            .unwrap();
        drop(c);
        let store = Store::open_readonly(&path).unwrap();
        store.inspect_schema().unwrap();
        assert!(
            store
                .connection
                .execute("INSERT INTO marker(value) VALUES (1)", [])
                .is_err()
        );
        remove(&path);
    }
}
