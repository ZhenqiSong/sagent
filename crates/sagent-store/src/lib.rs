//! Sagent SQLite 存储访问层。
//!
//! 作者：SongZQ
//! 创建日期：2026-08-29

use std::{fs, path::Path};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

pub mod message;
pub mod migration;
pub mod schema;
pub mod search;
pub mod session;
pub mod write;

pub use message::{MessageQuery, MessageWindow};
pub use migration::SCHEMA_VERSION;
pub use schema::DatabaseInfo;
pub use search::MessageSearchQuery;
pub use session::SessionListQuery;
pub use write::{NewMessage, NewSession, RetryCheckpoint, RewindCheckpoint, RewindResult};

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

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use rusqlite::Connection;

    use super::{MessageQuery, MessageSearchQuery, NewMessage, NewSession, SCHEMA_VERSION, Store};
    use sagent_types::{MessageId, SessionId};

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

    #[test]
    fn readwrite_store_migrates_and_persists_messages_with_fts() {
        let path = test_path("readwrite");
        remove_if_exists(&path);
        let session_id = SessionId::new("write-session");
        {
            let mut store = Store::open_readwrite(&path).expect("应能创建并迁移数据库");
            let info = store.inspect_schema().expect("应能读取迁移后的结构");
            assert_eq!(info.schema_version, Some(SCHEMA_VERSION));
            assert!(info.tables.iter().any(|table| table == "sessions"));
            assert!(info.tables.iter().any(|table| table == "messages_fts"));
            assert!(info.has_fts5);

            store
                .create_session(&NewSession {
                    id: session_id.clone(),
                    source: Some("tui".to_owned()),
                    model: Some("test-model".to_owned()),
                    title: Some("可写存储测试".to_owned()),
                    started_at: "2026-08-30T10:00:00Z".to_owned(),
                })
                .expect("应能创建会话");
            let message_id = store
                .append_message(&NewMessage::new(
                    session_id.clone(),
                    "user",
                    "使用 Rust 实现 FTS 搜索",
                    "2026-08-30T10:01:00Z",
                ))
                .expect("应能追加消息");
            assert_eq!(message_id.get(), 1);
            assert!(
                store
                    .update_session_activity(&session_id, "2026-08-30T10:02:00Z")
                    .expect("应能更新活动时间")
            );
        }

        let store = Store::open_readonly(&path).expect("应能重新以只读方式打开");
        let session = store
            .get_session(&session_id)
            .expect("应能读取已保存会话")
            .expect("已保存会话应存在");
        assert_eq!(session.message_count, 1);
        assert_eq!(session.title.as_deref(), Some("可写存储测试"));
        let hits = store
            .search_messages(&super::MessageSearchQuery::new("Rust"))
            .expect("FTS 触发器应同步写入索引");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0]
                .message_id
                .as_ref()
                .map(sagent_types::MessageId::get),
            Some(1)
        );
        remove_if_exists(&path);
    }

    #[test]
    fn failed_message_append_rolls_back_without_changing_session_count() {
        let path = test_path("write-rollback");
        remove_if_exists(&path);
        let session_id = SessionId::new("existing-session");
        let mut store = Store::open_readwrite(&path).expect("应能创建数据库");
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: None,
                model: None,
                title: None,
                started_at: "2026-08-30T10:00:00Z".to_owned(),
            })
            .expect("应能创建会话");

        let error = store
            .append_message(&NewMessage::new(
                SessionId::new("missing-session"),
                "user",
                "不会被写入",
                "2026-08-30T10:01:00Z",
            ))
            .expect_err("外键约束应拒绝不存在的会话");
        assert!(error.to_string().contains("写入消息"));
        let count: i64 = store
            .connection
            .query_row(
                "SELECT message_count FROM sessions WHERE id = ?1",
                [session_id.as_str()],
                |row| row.get(0),
            )
            .expect("应能读取消息计数");
        assert_eq!(count, 0);
        remove_if_exists(&path);
    }

    #[test]
    fn manages_session_lifecycle_and_list_visibility() {
        let path = test_path("lifecycle");
        remove_if_exists(&path);
        let session_id = SessionId::new("lifecycle-session");
        let mut store = Store::open_readwrite(&path).expect("应能创建数据库");
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: Some("cli".to_owned()),
                model: None,
                title: None,
                started_at: "2026-08-30T10:00:00Z".to_owned(),
            })
            .expect("应能创建会话");

        assert!(
            store
                .update_session_title(&session_id, Some("重命名后的会话"), "2026-08-30T10:01:00Z",)
                .expect("应能更新标题")
        );
        assert!(
            store
                .finish_session(&session_id, "completed", "2026-08-30T10:02:00Z")
                .expect("应能结束会话")
        );
        assert!(
            store
                .set_session_archived(&session_id, true, "2026-08-30T10:03:00Z")
                .expect("应能归档会话")
        );

        assert!(
            store
                .list_sessions(20, 0)
                .expect("应能读取会话列表")
                .is_empty(),
            "归档会话不应显示在普通列表"
        );
        let session = store
            .get_session(&session_id)
            .expect("应能精确读取归档会话")
            .expect("归档会话应保留");
        assert_eq!(session.title.as_deref(), Some("重命名后的会话"));
        assert_eq!(session.ended_at.as_deref(), Some("2026-08-30T10:02:00Z"));
        assert_eq!(session.end_reason.as_deref(), Some("completed"));

        assert!(
            store
                .set_session_archived(&session_id, false, "2026-08-30T10:04:00Z")
                .expect("应能取消归档")
        );
        assert!(
            store
                .set_session_hidden(&session_id, true, "2026-08-30T10:05:00Z")
                .expect("应能隐藏会话")
        );
        assert!(
            store
                .list_sessions(20, 0)
                .expect("应能读取会话列表")
                .is_empty(),
            "隐藏会话不应显示在普通列表"
        );
        assert!(
            store
                .set_session_hidden(&session_id, false, "2026-08-30T10:06:00Z")
                .expect("应能取消隐藏")
        );
        assert_eq!(
            store.list_sessions(20, 0).expect("应能读取会话列表").len(),
            1
        );

        assert!(
            !store
                .update_session_title(
                    &SessionId::new("missing-session"),
                    Some("不会写入"),
                    "2026-08-30T10:07:00Z",
                )
                .expect("未知会话不应导致 SQL 错误")
        );
        assert!(
            store
                .finish_session(&session_id, "", "2026-08-30T10:07:00Z")
                .is_err(),
            "结束原因不能为空"
        );
        remove_if_exists(&path);
    }

    #[test]
    fn rewinds_a_user_turn_and_preserves_auditable_history() {
        let path = test_path("rewind");
        remove_if_exists(&path);
        let session_id = SessionId::new("rewind-session");
        let mut store = Store::open_readwrite(&path).expect("应能创建数据库");
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: None,
                model: None,
                title: None,
                started_at: "2026-08-30T10:00:00Z".to_owned(),
            })
            .expect("应能创建会话");
        for (role, content, timestamp) in [
            ("user", "第一条提问", "2026-08-30T10:01:00Z"),
            ("assistant", "第一条回答", "2026-08-30T10:02:00Z"),
            ("user", "第二条 second 提问", "2026-08-30T10:03:00Z"),
            ("assistant", "第二条 second 回答", "2026-08-30T10:04:00Z"),
        ] {
            store
                .append_message(&NewMessage::new(
                    session_id.clone(),
                    role,
                    content,
                    timestamp,
                ))
                .expect("应能追加测试消息");
        }

        assert!(
            store
                .rewind_to_message(&session_id, MessageId::new(2), "2026-08-30T10:05:00Z")
                .is_err(),
            "assistant 消息不能作为回退目标"
        );
        let result = store
            .rewind_to_message(&session_id, MessageId::new(3), "2026-08-30T10:05:00Z")
            .expect("应能回退用户消息");
        assert_eq!(result.rewound_count, 2);
        assert_eq!(result.target_message.content, "第二条 second 提问");
        assert_eq!(result.new_head_id.as_ref().map(MessageId::get), Some(2));
        let checkpoint = result.checkpoint.clone();

        let active = store
            .get_messages(&session_id, &MessageQuery::default())
            .expect("应能读取活动消息");
        assert_eq!(
            active
                .iter()
                .map(|message| message.id.get())
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        let all_messages = store
            .get_messages(
                &session_id,
                &MessageQuery {
                    include_inactive: true,
                    ..MessageQuery::default()
                },
            )
            .expect("审计模式应能读取回退历史");
        assert_eq!(
            all_messages
                .iter()
                .map(|message| message.id.get())
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert!(
            store
                .search_messages(&MessageSearchQuery::new("second"))
                .expect("默认搜索应成功")
                .is_empty(),
            "默认搜索不能显示普通回退历史"
        );
        let mut audit_search = MessageSearchQuery::new("second");
        audit_search.include_inactive = true;
        assert_eq!(
            store
                .search_messages(&audit_search)
                .expect("审计搜索应成功")
                .len(),
            2
        );
        assert_eq!(
            store
                .get_session(&session_id)
                .expect("应能读取会话")
                .expect("会话应存在")
                .message_count,
            2
        );
        let rewind_count: i64 = store
            .connection
            .query_row(
                "SELECT rewind_count FROM sessions WHERE id = ?1",
                [session_id.as_str()],
                |row| row.get(0),
            )
            .expect("应能读取回退次数");
        assert_eq!(rewind_count, 1);

        assert_eq!(
            store
                .restore_rewound(&checkpoint, "2026-08-30T10:06:00Z")
                .expect("应能恢复回退消息"),
            2
        );
        assert_eq!(
            store
                .get_messages(&session_id, &MessageQuery::default())
                .expect("应能读取恢复后的活动消息")
                .iter()
                .map(|message| message.id.get())
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(
            store
                .get_session(&session_id)
                .expect("应能读取会话")
                .expect("会话应存在")
                .message_count,
            4
        );
        assert_eq!(
            store
                .search_messages(&MessageSearchQuery::new("second"))
                .expect("恢复后默认搜索应成功")
                .len(),
            2
        );

        let stale_checkpoint = store
            .rewind_to_message(&session_id, MessageId::new(3), "2026-08-30T10:07:00Z")
            .expect("应能再次回退用户消息")
            .checkpoint;
        store
            .append_message(&NewMessage::new(
                session_id.clone(),
                "user",
                "新的分支消息",
                "2026-08-30T10:08:00Z",
            ))
            .expect("应能追加新分支消息");
        let error = store
            .restore_rewound(&stale_checkpoint, "2026-08-30T10:09:00Z")
            .expect_err("新分支存在时必须拒绝恢复旧分支");
        assert!(error.to_string().contains("新的活动消息"));
        assert_eq!(
            store
                .get_messages(&session_id, &MessageQuery::default())
                .expect("拒绝恢复后仍应能读取活动消息")
                .iter()
                .map(|message| message.id.get())
                .collect::<Vec<_>>(),
            vec![1, 2, 5]
        );
        remove_if_exists(&path);
    }

    #[test]
    fn replaces_active_messages_without_losing_auditable_history() {
        let path = test_path("replace-active");
        remove_if_exists(&path);
        let session_id = SessionId::new("replace-session");
        let mut store = Store::open_readwrite(&path).expect("应能创建数据库");
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: None,
                model: None,
                title: None,
                started_at: "2026-08-30T10:00:00Z".to_owned(),
            })
            .expect("应能创建会话");
        for (role, content, timestamp) in [
            ("user", "oldbranch question", "2026-08-30T10:01:00Z"),
            ("assistant", "oldbranch answer", "2026-08-30T10:02:00Z"),
        ] {
            store
                .append_message(&NewMessage::new(
                    session_id.clone(),
                    role,
                    content,
                    timestamp,
                ))
                .expect("应能追加原活动消息");
        }

        let replacements = [
            NewMessage::new(
                session_id.clone(),
                "user",
                "newbranch question",
                "2026-08-30T10:03:00Z",
            ),
            NewMessage::new(
                session_id.clone(),
                "assistant",
                "newbranch answer",
                "2026-08-30T10:04:00Z",
            ),
        ];
        let inserted_ids = store
            .replace_active_messages(&session_id, &replacements, "2026-08-30T10:05:00Z")
            .expect("应能替换活动消息");
        assert_eq!(
            inserted_ids.iter().map(MessageId::get).collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert_eq!(
            store
                .get_messages(&session_id, &MessageQuery::default())
                .expect("应能读取新活动消息")
                .iter()
                .map(|message| message.id.get())
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert_eq!(
            store
                .get_messages(
                    &session_id,
                    &MessageQuery {
                        include_inactive: true,
                        ..MessageQuery::default()
                    },
                )
                .expect("审计模式应能读取所有分支")
                .len(),
            4
        );
        assert!(
            store
                .search_messages(&MessageSearchQuery::new("oldbranch"))
                .expect("默认搜索应成功")
                .is_empty()
        );
        let mut audit_search = MessageSearchQuery::new("oldbranch");
        audit_search.include_inactive = true;
        assert_eq!(
            store
                .search_messages(&audit_search)
                .expect("审计搜索应能找到旧分支")
                .len(),
            2
        );
        assert_eq!(
            store
                .get_session(&session_id)
                .expect("应能读取会话")
                .expect("会话应存在")
                .message_count,
            2
        );

        let invalid_replacements = [NewMessage::new(
            SessionId::new("other-session"),
            "user",
            "错误的会话消息",
            "2026-08-30T10:06:00Z",
        )];
        assert!(
            store
                .replace_active_messages(
                    &session_id,
                    &invalid_replacements,
                    "2026-08-30T10:06:00Z",
                )
                .is_err(),
            "跨会话替换必须在写入前失败"
        );
        assert_eq!(
            store
                .get_messages(&session_id, &MessageQuery::default())
                .expect("失败后活动消息不应改变")
                .iter()
                .map(|message| message.id.get())
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        remove_if_exists(&path);
    }

    #[test]
    fn retries_only_the_latest_assistant_message_with_a_checkpoint() {
        let path = test_path("retry");
        remove_if_exists(&path);
        let session_id = SessionId::new("retry-session");
        let mut store = Store::open_readwrite(&path).expect("应能创建数据库");
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: None,
                model: None,
                title: None,
                started_at: "2026-08-30T10:00:00Z".to_owned(),
            })
            .expect("应能创建会话");
        store
            .append_message(&NewMessage::new(
                session_id.clone(),
                "user",
                "retry question",
                "2026-08-30T10:01:00Z",
            ))
            .expect("应能写入用户消息");
        store
            .append_message(&NewMessage::new(
                session_id.clone(),
                "assistant",
                "oldretry answer",
                "2026-08-30T10:02:00Z",
            ))
            .expect("应能写入旧回答");

        assert!(
            store.prepare_retry(&session_id, MessageId::new(1)).is_err(),
            "用户消息不能重试"
        );
        let checkpoint = store
            .prepare_retry(&session_id, MessageId::new(2))
            .expect("最新 assistant 消息应可重试");
        assert_eq!(checkpoint.expected_active_head_id.get(), 2);
        assert_eq!(
            store
                .apply_retry(
                    &checkpoint,
                    &NewMessage::new(
                        session_id.clone(),
                        "assistant",
                        "newretry answer",
                        "2026-08-30T10:03:00Z",
                    ),
                    "2026-08-30T10:03:00Z",
                )
                .expect("应能写入重试回答")
                .get(),
            3
        );
        assert_eq!(
            store
                .get_messages(&session_id, &MessageQuery::default())
                .expect("应能读取活动分支")
                .iter()
                .map(|message| message.id.get())
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert!(
            store
                .search_messages(&MessageSearchQuery::new("oldretry"))
                .expect("默认搜索应成功")
                .is_empty()
        );
        let mut audit_search = MessageSearchQuery::new("oldretry");
        audit_search.include_inactive = true;
        assert_eq!(
            store
                .search_messages(&audit_search)
                .expect("审计搜索应找到旧回答")
                .len(),
            1
        );

        let stale_checkpoint = store
            .prepare_retry(&session_id, MessageId::new(3))
            .expect("新回答仍是最新 assistant 消息");
        store
            .append_message(&NewMessage::new(
                session_id.clone(),
                "user",
                "concurrent follow-up",
                "2026-08-30T10:04:00Z",
            ))
            .expect("应能模拟并发的新消息");
        assert!(
            store
                .apply_retry(
                    &stale_checkpoint,
                    &NewMessage::new(
                        session_id.clone(),
                        "assistant",
                        "must not persist",
                        "2026-08-30T10:05:00Z",
                    ),
                    "2026-08-30T10:05:00Z",
                )
                .is_err(),
            "检查点失效后必须拒绝重试"
        );
        assert_eq!(
            store
                .get_messages(&session_id, &MessageQuery::default())
                .expect("失败后活动分支不应改变")
                .iter()
                .map(|message| message.id.get())
                .collect::<Vec<_>>(),
            vec![1, 3, 4]
        );
        remove_if_exists(&path);
    }

    #[test]
    fn archives_compacted_history_but_keeps_it_searchable() {
        let path = test_path("compact");
        remove_if_exists(&path);
        let session_id = SessionId::new("compact-session");
        let mut store = Store::open_readwrite(&path).expect("应能创建数据库");
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: None,
                model: None,
                title: None,
                started_at: "2026-08-30T10:00:00Z".to_owned(),
            })
            .expect("应能创建会话");
        for (role, content, timestamp) in [
            ("user", "archiveknowledge question", "2026-08-30T10:01:00Z"),
            (
                "assistant",
                "archiveknowledge answer",
                "2026-08-30T10:02:00Z",
            ),
        ] {
            store
                .append_message(&NewMessage::new(
                    session_id.clone(),
                    role,
                    content,
                    timestamp,
                ))
                .expect("应能写入压缩前消息");
        }

        let compacted_messages = [NewMessage::compressed_summary(
            session_id.clone(),
            "assistant",
            "历史摘要：已讨论 archiveknowledge。",
            "2026-08-30T10:03:00Z",
        )];
        assert_eq!(
            store
                .archive_and_compact(&session_id, &compacted_messages, "2026-08-30T10:03:00Z")
                .expect("应能压缩活动上下文")
                .iter()
                .map(MessageId::get)
                .collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(
            store
                .get_messages_for_model(&session_id, &MessageQuery::default())
                .expect("默认读取应只返回压缩后的上下文")
                .iter()
                .map(|message| message.id.get())
                .collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(
            store
                .get_messages_for_model(&session_id, &MessageQuery::default())
                .expect("压缩摘要应保留给模型")
                .first()
                .and_then(|message| message.display_kind.as_deref()),
            Some("hidden")
        );
        assert_eq!(
            store
                .get_messages_for_display(&session_id, &MessageQuery::default())
                .expect("压缩展示读取应保留历史")
                .iter()
                .map(|message| message.id.get())
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            store
                .search_messages(&MessageSearchQuery::new("archiveknowledge"))
                .expect("默认搜索仍应命中压缩前知识")
                .len(),
            3
        );
        assert_eq!(
            store
                .get_session(&session_id)
                .expect("应能读取会话")
                .expect("会话应存在")
                .message_count,
            1
        );

        assert!(
            store
                .archive_and_compact(&session_id, &[], "2026-08-30T10:04:00Z")
                .is_err(),
            "空压缩结果必须在修改旧消息前被拒绝"
        );
        assert_eq!(
            store
                .get_messages(&session_id, &MessageQuery::default())
                .expect("失败后活动上下文不应改变")
                .len(),
            1
        );
        remove_if_exists(&path);
    }

    #[test]
    fn readwrite_store_upgrades_v1_schema_to_v2() {
        let path = test_path("migrate-v1");
        remove_if_exists(&path);
        let connection = Connection::open(&path).expect("应能创建 v1 fixture");
        connection
            .execute_batch(
                "CREATE TABLE schema_version (version INTEGER NOT NULL);
                 INSERT INTO schema_version(version) VALUES (1);
                 CREATE TABLE sessions (id TEXT PRIMARY KEY);",
            )
            .expect("应能创建 v1 结构");
        drop(connection);

        let store = Store::open_readwrite(&path).expect("应能升级 v1 数据库");
        assert_eq!(
            store.inspect_schema().expect("应能读取结构").schema_version,
            Some(2)
        );
        let has_rewind_count: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions')
                 WHERE name = 'rewind_count'",
                [],
                |row| row.get(0),
            )
            .expect("应能读取升级后的列");
        assert_eq!(has_rewind_count, 1);
        remove_if_exists(&path);
    }
}
