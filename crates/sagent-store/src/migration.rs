//! Sagent SQLite 数据库的版本化初始化与迁移。
//!
//! 作者：SongZQ

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

/// 当前 Sagent 自有数据库结构版本。
pub const SCHEMA_VERSION: i64 = 3;

/// 在当前事务中将 v2 结构扩展为 v3。
fn migrate_v2_to_v3(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS session_generations (
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                generation INTEGER NOT NULL CHECK (generation >= 0),
                system_hash TEXT NOT NULL,
                tool_schema_hash TEXT NOT NULL,
                model_id TEXT NOT NULL,
                profile_revision TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (session_id, generation)
             );

             CREATE TABLE IF NOT EXISTS turns (
                turn_id TEXT PRIMARY KEY NOT NULL,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                generation INTEGER NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('running', 'completed', 'interrupted', 'failed')),
                user_message_id INTEGER REFERENCES messages(id),
                assistant_message_id INTEGER REFERENCES messages(id),
                started_at TEXT NOT NULL,
                completed_at TEXT,
                outcome_json TEXT,
                FOREIGN KEY (session_id, generation)
                    REFERENCES session_generations(session_id, generation)
             );

             CREATE TABLE IF NOT EXISTS daemon_events (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                turn_id TEXT REFERENCES turns(turn_id) ON DELETE SET NULL,
                event_type TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at TEXT NOT NULL
             );

             CREATE INDEX IF NOT EXISTS idx_turns_session_started
             ON turns(session_id, started_at);

             CREATE INDEX IF NOT EXISTS idx_daemon_events_session_sequence
             ON daemon_events(session_id, sequence);

             UPDATE schema_version SET version = 3;",
        )
        .context("从 v2 升级至 v3 失败")?;
    Ok(())
}

/// 将数据库迁移到当前版本。
///
/// v1 初始化 Sagent 自己的表与 FTS5 索引，v2 增加会话回退计数。它不尝试
/// 兼容或修改 Hermes 的 state.db；读取 Hermes 数据仍应使用只读 Store。
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
        Some(3) => {}
        Some(2) => {
            migrate_v2_to_v3(&transaction)?;
        }
        Some(1) => {
            transaction
                .execute_batch(
                    "ALTER TABLE sessions
                     ADD COLUMN rewind_count INTEGER NOT NULL DEFAULT 0;
                     UPDATE schema_version SET version = 2;",
                )
                .context("从 v1 升级至 v2 失败")?;
            migrate_v2_to_v3(&transaction)?;
        }
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
                        rewind_count INTEGER NOT NULL DEFAULT 0,
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

                     INSERT INTO schema_version(version) VALUES (2);",
                )
                .context("创建 v1 数据库结构失败")?;
            migrate_v2_to_v3(&transaction)?;
        }
    }
    transaction.commit().context("提交数据库迁移事务失败")
}
