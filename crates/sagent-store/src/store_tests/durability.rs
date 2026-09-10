//! Store 迁移、Turn 原子持久化和 Profile 隔离契约。

use super::*;

#[test]
fn readwrite_store_upgrades_v1_schema_to_v3() {
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
        Some(3)
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
    for table in ["session_generations", "turns", "daemon_events"] {
        let exists: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .expect("应能读取 v3 新表");
        assert_eq!(exists, 1, "缺少 v3 表：{table}");
    }
    remove_if_exists(&path);
}

#[test]
fn readwrite_store_upgrades_v2_schema_to_v3() {
    let path = test_path("migrate-v2");
    remove_if_exists(&path);
    let connection = Connection::open(&path).expect("应能创建 v2 fixture");
    connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE schema_version (version INTEGER NOT NULL);
                 INSERT INTO schema_version(version) VALUES (2);
                 CREATE TABLE sessions (id TEXT PRIMARY KEY, rewind_count INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id), role TEXT NOT NULL, content TEXT NOT NULL, timestamp TEXT NOT NULL);",
            )
            .expect("应能创建 v2 结构");
    drop(connection);

    let store = Store::open_readwrite(&path).expect("应能升级 v2 数据库");
    assert_eq!(
        store.inspect_schema().expect("应能读取结构").schema_version,
        Some(3)
    );
    let table_count: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('session_generations', 'turns', 'daemon_events')",
                [],
                |row| row.get(0),
            )
            .expect("应能读取 v3 表");
    assert_eq!(table_count, 3);
    remove_if_exists(&path);
}

#[test]
fn begin_turn_atomically_persists_user_message_turn_and_events() {
    let path = test_path("begin-turn");
    remove_if_exists(&path);
    let mut store = Store::open_readwrite(&path).expect("应能创建 Store");
    let session_id = SessionId::new("session-begin");
    store
        .create_session(&NewSession {
            id: session_id.clone(),
            source: Some("tui".into()),
            model: Some("mock".into()),
            title: None,
            started_at: "2026-09-02T00:00:00Z".into(),
        })
        .expect("应能创建会话");
    store
        .create_generation(&NewGeneration {
            session_id: session_id.clone(),
            generation: 0,
            system_hash: "sha256:system".into(),
            tool_schema_hash: "sha256:tools".into(),
            model_id: "mock".into(),
            profile_revision: "default".into(),
            created_at: "2026-09-02T00:00:00Z".into(),
        })
        .expect("应能创建 generation");

    let turn_id = TurnId::new();
    let message_id = store
        .begin_turn(
            &StartTurn {
                turn_id,
                session_id: session_id.clone(),
                generation: 0,
                started_at: "2026-09-02T00:00:01Z".into(),
            },
            &NewMessage::new(
                session_id.clone(),
                "user",
                "开始执行",
                "2026-09-02T00:00:01Z",
            ),
        )
        .expect("应能开始 Turn");

    assert_eq!(message_id.get(), 1);
    let (status, stored_message_id, count): (String, i64, i64) = store
        .connection
        .query_row(
            "SELECT t.status, t.user_message_id, s.message_count
                 FROM turns t JOIN sessions s ON s.id = t.session_id
                 WHERE t.turn_id = ?1",
            [turn_id.as_uuid().to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("应能读取 Turn");
    assert_eq!(status, "running");
    assert_eq!(stored_message_id, message_id.get());
    assert_eq!(count, 1);

    let event_count: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM daemon_events WHERE turn_id = ?1",
            [turn_id.as_uuid().to_string()],
            |row| row.get(0),
        )
        .expect("应能读取 Turn 事件");
    assert_eq!(event_count, 2);
    remove_if_exists(&path);
}

#[test]
fn commit_tool_result_persists_message_and_events_atomically() {
    let path = test_path("tool-result");
    remove_if_exists(&path);
    let mut store = Store::open_readwrite(&path).expect("应能创建 Store");
    let session_id = SessionId::new("session-tool");
    store
        .create_session(&NewSession {
            id: session_id.clone(),
            source: Some("tui".into()),
            model: Some("mock".into()),
            title: None,
            started_at: "2026-09-02T00:00:00Z".into(),
        })
        .expect("应能创建会话");
    store
        .create_generation(&NewGeneration {
            session_id: session_id.clone(),
            generation: 0,
            system_hash: "sha256:system".into(),
            tool_schema_hash: "sha256:tools".into(),
            model_id: "mock".into(),
            profile_revision: "default".into(),
            created_at: "2026-09-02T00:00:00Z".into(),
        })
        .expect("应能创建 generation");
    let turn_id = TurnId::new();
    store
        .begin_turn(
            &StartTurn {
                turn_id,
                session_id: session_id.clone(),
                generation: 0,
                started_at: "2026-09-02T00:00:01Z".into(),
            },
            &NewMessage::new(
                session_id.clone(),
                "user",
                "执行工具",
                "2026-09-02T00:00:01Z",
            ),
        )
        .expect("应能开始 Turn");
    let tool_id = "call-001";
    let result_id = store
        .commit_tool_result(
            &turn_id,
            &NewMessage {
                session_id: session_id.clone(),
                role: "tool".into(),
                content: "结果".into(),
                timestamp: "2026-09-02T00:00:02Z".into(),
                tool_call_id: Some(tool_id.into()),
                tool_name: Some("terminal".into()),
                tool_calls: None,
                reasoning: None,
                finish_reason: Some("tool_completed".into()),
                display_kind: None,
                display_metadata: None,
            },
            "2026-09-02T00:00:02Z",
        )
        .expect("应能提交工具结果");
    assert_eq!(result_id.get(), 2);
    let count: i64 = store
        .connection
        .query_row(
            "SELECT message_count FROM sessions WHERE id = ?1",
            [session_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
    let events: i64 = store.connection.query_row("SELECT COUNT(*) FROM daemon_events WHERE turn_id = ?1 AND event_type IN ('tool.completed', 'message.committed')", [turn_id.as_uuid().to_string()], |row| row.get(0)).unwrap();
    assert_eq!(events, 3);
    assert!(
        store
            .commit_tool_result(
                &turn_id,
                &NewMessage::new(
                    session_id.clone(),
                    "assistant",
                    "重复",
                    "2026-09-02T00:00:03Z"
                ),
                "2026-09-02T00:00:03Z"
            )
            .is_err()
    );
    remove_if_exists(&path);
}

#[test]
fn events_since_filters_by_sequence_and_reports_latest() {
    let path = test_path("events-since");
    remove_if_exists(&path);
    let mut store = Store::open_readwrite(&path).unwrap();
    let session_id = SessionId::new("session-events");
    store
        .create_session(&NewSession {
            id: session_id.clone(),
            source: None,
            model: None,
            title: None,
            started_at: "2026-09-02T00:00:00Z".into(),
        })
        .unwrap();
    store
        .create_generation(&NewGeneration {
            session_id: session_id.clone(),
            generation: 0,
            system_hash: "system".into(),
            tool_schema_hash: "tools".into(),
            model_id: "mock".into(),
            profile_revision: "default".into(),
            created_at: "2026-09-02T00:00:00Z".into(),
        })
        .unwrap();
    let turn_id = TurnId::new();
    store
        .begin_turn(
            &StartTurn {
                turn_id,
                session_id: session_id.clone(),
                generation: 0,
                started_at: "2026-09-02T00:00:01Z".into(),
            },
            &NewMessage::new(session_id.clone(), "user", "事件", "2026-09-02T00:00:01Z"),
        )
        .unwrap();
    let events = store
        .events_since(&EventQuery {
            session_id: session_id.clone(),
            after_sequence: EventSequence::default(),
            limit: 1,
        })
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "turn.started");
    let latest = store.latest_event_sequence(&session_id).unwrap().unwrap();
    assert_eq!(latest.get(), 2);
    assert!(
        store
            .events_since(&EventQuery {
                session_id,
                after_sequence: latest,
                limit: 10
            })
            .unwrap()
            .is_empty()
    );
    remove_if_exists(&path);
}

#[test]
fn turn_persistence_survives_reopen_and_keeps_fts_and_replay_consistent() {
    let path = test_path("turn-e2e");
    remove_if_exists(&path);
    let session_id = SessionId::new("session-e2e");
    let turn_id = TurnId::new();
    {
        let mut store = Store::open_readwrite(&path).unwrap();
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: Some("tui".into()),
                model: Some("mock".into()),
                title: None,
                started_at: "2026-09-02T00:00:00Z".into(),
            })
            .unwrap();
        store
            .create_generation(&NewGeneration {
                session_id: session_id.clone(),
                generation: 0,
                system_hash: "system".into(),
                tool_schema_hash: "tools".into(),
                model_id: "mock".into(),
                profile_revision: "default".into(),
                created_at: "2026-09-02T00:00:00Z".into(),
            })
            .unwrap();
        store
            .begin_turn(
                &StartTurn {
                    turn_id,
                    session_id: session_id.clone(),
                    generation: 0,
                    started_at: "2026-09-02T00:00:01Z".into(),
                },
                &NewMessage::new(
                    session_id.clone(),
                    "user",
                    "查询 Rust 文件",
                    "2026-09-02T00:00:01Z",
                ),
            )
            .unwrap();
        store
            .commit_tool_result(
                &turn_id,
                &NewMessage {
                    session_id: session_id.clone(),
                    role: "tool".into(),
                    content: "src/main.rs".into(),
                    timestamp: "2026-09-02T00:00:02Z".into(),
                    tool_call_id: Some("call-e2e".into()),
                    tool_name: Some("terminal".into()),
                    tool_calls: None,
                    reasoning: None,
                    finish_reason: None,
                    display_kind: None,
                    display_metadata: None,
                },
                "2026-09-02T00:00:02Z",
            )
            .unwrap();
        store
            .complete_turn(
                &turn_id,
                &NewMessage::new(
                    session_id.clone(),
                    "assistant",
                    "找到 Rust 文件",
                    "2026-09-02T00:00:03Z",
                ),
                "2026-09-02T00:00:03Z",
            )
            .unwrap();
    }
    let store = Store::open_readonly(&path).unwrap();
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(
        store
            .search_messages(&MessageSearchQuery {
                query: "Rust".into(),
                session_id: Some(session_id.clone()),
                include_inactive: false,
                limit: 20
            })
            .unwrap()
            .len(),
        2
    );
    let events = store
        .events_since(&EventQuery {
            session_id: session_id.clone(),
            after_sequence: EventSequence::default(),
            limit: 20,
        })
        .unwrap();
    assert_eq!(events.len(), 6);
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert_eq!(
        store
            .latest_event_sequence(&session_id)
            .unwrap()
            .unwrap()
            .get(),
        6
    );
    remove_if_exists(&path);
}

#[test]
fn event_queries_are_isolated_between_profile_databases() {
    let path_a = test_path("profile-a");
    let path_b = test_path("profile-b");
    remove_if_exists(&path_a);
    remove_if_exists(&path_b);
    for (path, id) in [(&path_a, "session-a"), (&path_b, "session-b")] {
        let mut store = Store::open_readwrite(path).unwrap();
        let session_id = SessionId::new(id);
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: None,
                model: None,
                title: None,
                started_at: "2026-09-02T00:00:00Z".into(),
            })
            .unwrap();
        store
            .create_generation(&NewGeneration {
                session_id: session_id.clone(),
                generation: 0,
                system_hash: "system".into(),
                tool_schema_hash: "tools".into(),
                model_id: "mock".into(),
                profile_revision: "default".into(),
                created_at: "2026-09-02T00:00:00Z".into(),
            })
            .unwrap();
        let turn_id = TurnId::new();
        store
            .begin_turn(
                &StartTurn {
                    turn_id,
                    session_id: session_id.clone(),
                    generation: 0,
                    started_at: "2026-09-02T00:00:01Z".into(),
                },
                &NewMessage::new(session_id, "user", id, "2026-09-02T00:00:01Z"),
            )
            .unwrap();
    }
    let store_b = Store::open_readonly(&path_b).unwrap();
    assert!(
        store_b
            .events_since(&EventQuery {
                session_id: SessionId::new("session-a"),
                after_sequence: EventSequence::default(),
                limit: 10
            })
            .is_err()
    );
    remove_if_exists(&path_a);
    remove_if_exists(&path_b);
}
