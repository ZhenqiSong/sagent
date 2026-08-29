//! 会话列表的只读查询。
//!
//! 此处实现 Python SessionDB.list_sessions_rich 的基础查询契约；压缩链投影、
//! 置顶回填和复杂筛选会在调用方明确需要时逐步加入。
//! 作者：SongZQ

use anyhow::{Context, Result};
use rusqlite::params;
use sagent_types::{SessionId, SessionSummary};

use crate::Store;

impl Store {
    /// 按最后活动时间倒序读取未归档、未隐藏的会话摘要。
    ///
    /// limit 与 offset 用于 TUI 会话选择器的分页。预览取第一条用户消息的前
    /// 60 个字符；最后活动时间优先使用最新消息的时间，没有消息时回退到会话开始时间。
    pub fn list_sessions(&self, limit: u32, offset: u32) -> Result<Vec<SessionSummary>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT
                    s.id,
                    s.source,
                    s.model,
                    s.title,
                    s.started_at,
                    s.ended_at,
                    s.end_reason,
                    COALESCE(
                        (SELECT MAX(m.timestamp)
                         FROM messages AS m
                         WHERE m.session_id = s.id),
                        s.started_at
                    ) AS last_active,
                    COALESCE(
                        (SELECT substr(m.content, 1, 60)
                         FROM messages AS m
                         WHERE m.session_id = s.id
                           AND m.role = 'user'
                           AND m.content IS NOT NULL
                         ORDER BY m.timestamp ASC, m.id ASC
                         LIMIT 1),
                        ''
                    ) AS preview,
                    s.message_count
                 FROM sessions AS s
                 WHERE s.archived = 0 AND s.hidden = 0
                 ORDER BY last_active DESC, s.started_at DESC, s.id DESC
                 LIMIT ?1 OFFSET ?2",
            )
            .context("准备会话列表查询失败")?;

        let rows = statement
            .query_map(params![i64::from(limit), i64::from(offset)], |row| {
                Ok(SessionSummary {
                    id: SessionId::new(row.get::<_, String>(0)?),
                    source: row.get(1)?,
                    model: row.get(2)?,
                    title: row.get(3)?,
                    started_at: row.get(4)?,
                    ended_at: row.get(5)?,
                    end_reason: row.get(6)?,
                    last_active: row.get(7)?,
                    preview: row.get(8)?,
                    message_count: row.get(9)?,
                })
            })
            .context("执行会话列表查询失败")?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("读取会话列表记录失败")
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use rusqlite::Connection;

    use crate::Store;

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sagent-sessions-{name}-{}.db", std::process::id()))
    }

    fn remove(path: &std::path::Path) {
        let _ = fs::remove_file(path);
    }

    fn create_fixture(path: &std::path::Path) {
        let connection = Connection::open(path).expect("应能创建测试数据库");
        connection
            .execute_batch(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    source TEXT,
                    model TEXT,
                    title TEXT,
                    started_at TEXT,
                    ended_at TEXT,
                    end_reason TEXT,
                    message_count INTEGER NOT NULL,
                    archived INTEGER NOT NULL DEFAULT 0,
                    hidden INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE TABLE messages (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    content TEXT,
                    timestamp TEXT
                 );",
            )
            .expect("应能创建会话测试表");

        connection
            .execute_batch(
                "INSERT INTO sessions VALUES
                    ('old', 'cli', 'model-a', '旧会话', '2026-08-29T10:00:00Z',
                     NULL, NULL, 2, 0, 0),
                    ('recent', 'tui', 'model-b', '新会话', '2026-08-29T11:00:00Z',
                     NULL, NULL, 3, 0, 0),
                    ('archived', 'cli', 'model-c', '已归档', '2026-08-29T12:00:00Z',
                     NULL, NULL, 1, 1, 0),
                    ('hidden', 'cli', 'model-d', '隐藏会话', '2026-08-29T13:00:00Z',
                     NULL, NULL, 1, 0, 1);
                 INSERT INTO messages VALUES
                    (1, 'old', 'user', '这是旧会话的第一条提问', '2026-08-29T10:01:00Z'),
                    (2, 'old', 'assistant', '旧回复', '2026-08-29T10:02:00Z'),
                    (3, 'recent', 'user', '这是新会话的第一条提问', '2026-08-29T11:01:00Z'),
                    (4, 'recent', 'assistant', '新回复', '2026-08-29T14:00:00Z');",
            )
            .expect("应能插入会话测试数据");
    }

    #[test]
    fn lists_visible_sessions_by_last_activity_with_preview() {
        let path = test_path("list");
        remove(&path);
        create_fixture(&path);

        let sessions = Store::open_readonly(&path)
            .expect("应能只读打开 fixture")
            .list_sessions(20, 0)
            .expect("应能读取会话列表");

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id.as_str(), "recent");
        assert_eq!(
            sessions[0].last_active.as_deref(),
            Some("2026-08-29T14:00:00Z")
        );
        assert_eq!(
            sessions[0].preview.as_deref(),
            Some("这是新会话的第一条提问")
        );
        assert_eq!(sessions[1].id.as_str(), "old");

        remove(&path);
    }

    #[test]
    fn applies_limit_and_offset_after_sorting() {
        let path = test_path("page");
        remove(&path);
        create_fixture(&path);

        let sessions = Store::open_readonly(&path)
            .expect("应能只读打开 fixture")
            .list_sessions(1, 1)
            .expect("应能读取会话列表");

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id.as_str(), "old");

        remove(&path);
    }
}
