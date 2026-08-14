//! sagent-session 真实 SQLite 初始化和 migration 测试。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 3 SQLite 测试

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use sagent_config::{DatabaseConfig, SynchronousMode};
use sagent_session::{DatabaseConnection, DatabaseError, Migration};

static TEST_ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let suffix =
            SystemTime::now().duration_since(UNIX_EPOCH).expect("系统时间应有效").as_nanos();
        let sequence = TEST_ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sagent-session-{}-{suffix}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("应创建测试目录");
        Self(path)
    }

    fn db(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn config() -> DatabaseConfig {
    DatabaseConfig {
        path: None,
        busy_timeout_ms: 2_500,
        synchronous: SynchronousMode::Full,
    }
}

#[test]
fn first_open_creates_database_and_complete_schema() {
    let root = TestRoot::new();
    let path = root.db("nested/state.db");
    let database = DatabaseConnection::open(&path, &config()).expect("首次打开应成功");
    assert_eq!(database.path(), Path::new(&path));
    assert_eq!(database.schema_version(), 2);
    assert!(path.exists());
    for table in ["schema_meta", "sessions", "messages"] {
        assert!(database.table_exists(table).expect("应查询表"));
    }
    for index in ["idx_sessions_updated_at", "idx_messages_session_sequence"] {
        assert!(database.index_exists(index).expect("应查询索引"));
    }
    assert_eq!(
        database.table_columns("sessions").expect("应查询列"),
        vec![
            "id",
            "source",
            "title",
            "cwd",
            "status",
            "metadata_json",
            "created_at",
            "updated_at",
            "message_count",
            "revision"
        ]
    );
}

#[test]
fn reopen_is_idempotent_and_preserves_committed_data() {
    let root = TestRoot::new();
    let path = root.db("state.db");
    let first = DatabaseConnection::open(&path, &config()).expect("首次打开应成功");
    drop(first);
    let connection = Connection::open(&path).expect("测试连接应打开");
    connection
        .execute(
            "INSERT INTO sessions(id, source, status, created_at, updated_at) \
             VALUES('sess_1', 'test', 'active', 't1', 't1')",
            [],
        )
        .expect("应插入测试 session");
    drop(connection);

    let reopened = DatabaseConnection::open(&path, &config()).expect("重复打开应成功");
    assert_eq!(reopened.schema_version(), 2);
    let verify = Connection::open(&path).expect("验证连接应打开");
    let count: i64 = verify
        .query_row(
            "SELECT count(*) FROM sessions WHERE id = 'sess_1'",
            [],
            |row| row.get(0),
        )
        .expect("应读取已提交数据");
    assert_eq!(count, 1);
}

#[test]
fn configured_pragmas_are_applied_and_read_back() {
    let root = TestRoot::new();
    let database = DatabaseConnection::open(root.db("state.db"), &config()).expect("应打开");
    let pragma = database.pragma_state().expect("应读取 pragma");
    assert!(pragma.foreign_keys);
    assert_eq!(pragma.journal_mode, "wal");
    assert_eq!(pragma.busy_timeout_ms, 2_500);
    assert_eq!(pragma.synchronous, 2);
}

#[test]
fn foreign_key_enforcement_is_real_not_only_schema_text() {
    let root = TestRoot::new();
    let database = DatabaseConnection::open(root.db("state.db"), &config()).expect("应打开");
    let connection = Connection::open(database.path()).expect("验证连接应打开");
    let result = connection.execute(
        "INSERT INTO messages(id, session_id, sequence, role, content_json, created_at) \
         VALUES('msg_1', 'missing', 1, 'user', '[]', 't1')",
        [],
    );
    assert!(result.is_err(), "不存在的 Session 不应允许写入消息");
}

#[test]
fn failed_migration_rolls_back_version_and_schema_changes() {
    static BROKEN_MIGRATIONS: &[Migration] = &[
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
        Migration {
            version: 3,
            name: "0003_broken",
            sql: "CREATE TABLE should_rollback (id TEXT); INVALID SQL;",
        },
    ];
    let root = TestRoot::new();
    let path = root.db("state.db");
    DatabaseConnection::open(&path, &config()).expect("应先创建标准 v2 schema");
    let error = DatabaseConnection::open_with_migrations(&path, &config(), BROKEN_MIGRATIONS)
        .expect_err("损坏 migration 应失败");
    assert!(matches!(error, DatabaseError::Migration { version: 3, .. }));

    let verify = Connection::open(&path).expect("验证连接应打开");
    let version: String = verify
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'current_version'",
            [],
            |row| row.get(0),
        )
        .expect("旧版本应保留");
    assert_eq!(version, "2");
    let rolled_back: i64 = verify
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'should_rollback'",
            [],
            |row| row.get(0),
        )
        .expect("应查询 rollback 表");
    assert_eq!(rolled_back, 0);
    DatabaseConnection::open(&path, &config()).expect("修复后应可再次打开");
}

#[test]
fn existing_non_sagent_database_is_rejected_without_modification() {
    let root = TestRoot::new();
    let path = root.db("state.db");
    let connection = Connection::open(&path).expect("应打开旧数据库");
    connection
        .execute("CREATE TABLE legacy_state (id INTEGER PRIMARY KEY)", [])
        .expect("应创建 legacy 表");
    drop(connection);

    let error = DatabaseConnection::open(&path, &config()).expect_err("旧 schema 应拒绝");
    assert!(matches!(error, DatabaseError::Unsupported { .. }));
    let verify = Connection::open(&path).expect("应重新打开旧数据库");
    let count: i64 = verify
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'legacy_state'",
            [],
            |row| row.get(0),
        )
        .expect("应查询 legacy 表");
    assert_eq!(count, 1);
}

#[test]
fn future_schema_version_is_rejected() {
    let root = TestRoot::new();
    let path = root.db("state.db");
    let connection = Connection::open(&path).expect("应打开数据库");
    connection
        .execute_batch(
            "CREATE TABLE schema_meta(key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL); \
             INSERT INTO schema_meta(key, value) VALUES('current_version', '99');",
        )
        .expect("应写入 future schema");
    drop(connection);
    let error = DatabaseConnection::open(&path, &config()).expect_err("future schema 应拒绝");
    assert!(matches!(error, DatabaseError::Unsupported { .. }));
}
