//! Store 打开、只读边界和基础写入契约。

use super::*;

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
fn batch_append_is_atomic_and_updates_the_session_once() {
    // 批量导入承诺“全写入或全不写入”：该契约既让 10k/100k fixture 可行，也防止
    // 未来导入器因为半批成功而留下与 message_count 不一致的会话。
    let path = test_path("batch-append");
    remove_if_exists(&path);
    let session_id = SessionId::new("batch-session");
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

    let ids = store
        .append_messages(&[
            NewMessage::new(
                session_id.clone(),
                "user",
                "first batch message",
                "2026-08-30T10:01:00Z",
            ),
            NewMessage::new(
                session_id.clone(),
                "assistant",
                "second batch message",
                "2026-08-30T10:02:00Z",
            ),
        ])
        .expect("同一会话的整批消息应成功");
    assert_eq!(ids.len(), 2);
    let session = store
        .get_session(&session_id)
        .expect("应能读取会话")
        .expect("会话仍应存在");
    assert_eq!(session.message_count, 2);
    assert_eq!(session.last_active.as_deref(), Some("2026-08-30T10:02:00Z"));

    let error = store
        .append_messages(&[
            NewMessage::new(
                session_id.clone(),
                "user",
                "valid prefix must roll back",
                "2026-08-30T10:03:00Z",
            ),
            NewMessage::new(
                SessionId::new("other-session"),
                "assistant",
                "invalid batch member",
                "2026-08-30T10:04:00Z",
            ),
        ])
        .expect_err("跨会话批量写入必须在开始事务前被拒绝");
    assert!(error.to_string().contains("同一会话"));
    assert_eq!(
        store
            .get_session(&session_id)
            .unwrap()
            .expect("原会话仍存在")
            .message_count,
        2
    );
    remove_if_exists(&path);
}
