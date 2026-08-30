//! Sagent SQLite 数据库的版本化初始化与迁移。
//!
//! 作者：SongZQ

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

/// 当前 Sagent 自有数据库结构版本。
pub const SCHEMA_VERSION: i64 = 1;

/// 将数据库迁移到当前版本。
///
/// v1 仅初始化 Sagent 自己的表与 FTS5 索引。它不尝试兼容或修改 Hermes 的
/// state.db；读取 Hermes 数据仍应使用只读 Store。
pub(crate) fn migrate(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction().context("开始数据库迁移事务失败")?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER NOT NULL
             );",
        )
        .context("创建 schema_version 表失败")?;
    let version = transaction
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
            row.get::<_, i64>(0)
        })
        .optional()
        .context("读取数据库结构版本失败")?;

    match version {
        Some(version) if version > SCHEMA_VERSION => {
            anyhow::bail!("数据库结构版本 {version} 高于当前程序支持的版本 {SCHEMA_VERSION}");
        }
        Some(SCHEMA_VERSION) => {}
        Some(version) => {
            anyhow::bail!("暂不支持从数据库结构版本 {version} 自动迁移");
        }
        None => {
            transaction
                .execute_batch(
                    "CREATE TABLE sessions (
                        id TEXT PRIMARY KEY NOT NULL,
                        source TEXT,
                        model TEXT,
                        title TEXT,
                        started_at TEXT NOT NULL,
                        ended_at TEXT,
                        end_reason TEXT,
                        last_activity_at TEXT,
                        updated_at TEXT,
                        message_count INTEGER NOT NULL DEFAULT 0,
                        archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
                        hidden INTEGER NOT NULL DEFAULT 0 CHECK (hidden IN (0, 1))
                     );

                     CREATE TABLE messages (
                        id INTEGER PRIMARY KEY,
                        session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                        role TEXT NOT NULL,
                        content TEXT NOT NULL,
                        timestamp TEXT NOT NULL,
                        tool_call_id TEXT,
                        tool_name TEXT,
                        tool_calls TEXT,
                        reasoning TEXT,
                        finish_reason TEXT,
                        display_kind TEXT,
                        display_metadata TEXT,
                        active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
                        compacted INTEGER NOT NULL DEFAULT 0 CHECK (compacted IN (0, 1))
                     );

                     CREATE INDEX idx_messages_session_id_id
                     ON messages(session_id, id);

                     CREATE VIRTUAL TABLE messages_fts USING fts5(
                        content,
                        content='messages',
                        content_rowid='id'
                     );

                     CREATE TRIGGER messages_fts_insert
                     AFTER INSERT ON messages
                     BEGIN
                        INSERT INTO messages_fts(rowid, content)
                        VALUES (new.id, new.content);
                     END;

                     CREATE TRIGGER messages_fts_delete
                     AFTER DELETE ON messages
                     BEGIN
                        INSERT INTO messages_fts(messages_fts, rowid, content)
                        VALUES ('delete', old.id, old.content);
                     END;

                     CREATE TRIGGER messages_fts_update
                     AFTER UPDATE OF content ON messages
                     BEGIN
                        INSERT INTO messages_fts(messages_fts, rowid, content)
                        VALUES ('delete', old.id, old.content);
                        INSERT INTO messages_fts(rowid, content)
                        VALUES (new.id, new.content);
                     END;

                     INSERT INTO schema_version(version) VALUES (1);",
                )
                .context("创建 v1 数据库结构失败")?;
        }
    }
    transaction.commit().context("提交数据库迁移事务失败")
}
