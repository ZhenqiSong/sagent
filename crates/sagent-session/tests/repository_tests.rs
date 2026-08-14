//! sagent-session Repository 真实 SQLite 集成测试。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 4 Repository 测试

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use sagent_config::{DatabaseConfig, SynchronousMode};
use sagent_session::{
    AppendMessage, CreateSession, DatabaseConnection, ListSessions, MessageRange, Repository,
    RepositoryError, SessionCursor, MAX_LIST_LIMIT,
};
use sagent_types::ids::SessionId;
use sagent_types::message::{ContentPart, Role};
use sagent_types::session::SessionStatus;

static TEST_ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let timestamp =
            SystemTime::now().duration_since(UNIX_EPOCH).expect("系统时间应有效").as_nanos();
        let sequence = TEST_ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sagent-repository-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("应创建测试目录");
        Self(path)
    }

    fn database_path(&self) -> PathBuf {
        self.0.join("state.db")
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
        busy_timeout_ms: 5_000,
        synchronous: SynchronousMode::Full,
    }
}

fn repository(path: &PathBuf) -> Repository {
    Repository::new(DatabaseConnection::open(path, &config()).expect("数据库应成功打开"))
}

#[test]
fn create_get_append_get_messages_and_resume_are_consistent() {
    let root = TestRoot::new();
    let path = root.database_path();
    let mut repo = repository(&path);
    let mut create = CreateSession::new("stdio");
    create.title = Some("Repository test".to_string());
    create.cwd = Some("/tmp/workspace".to_string());
    create.metadata.insert("kind".to_string(), serde_json::json!("test"));

    let session = repo.create_session(create).expect("应创建 Session");
    assert_eq!(session.status, SessionStatus::Active);
    assert_eq!(session.message_count, 0);
    assert_eq!(session.revision, 0);
    assert_eq!(
        repo.get_session(&session.id).expect("应查询 Session").unwrap().id,
        session.id
    );

    let first = repo
        .append_message(&session.id, AppendMessage::text(Role::User, "first"))
        .expect("应追加第一条消息");
    assert_eq!(first.sequence, 1);
    assert_eq!(first.session_id, session.id);
    let second = repo
        .append_message(&session.id, AppendMessage::text(Role::Assistant, "second"))
        .expect("应追加第二条消息");
    assert_eq!(second.sequence, 2);

    let updated = repo
        .get_session(&session.id)
        .expect("应查询更新后的 Session")
        .expect("Session 应存在");
    assert_eq!(updated.message_count, 2);
    assert_eq!(updated.revision, 2);
    assert!(updated.updated_at >= session.updated_at);

    let messages = repo.get_messages(&session.id, MessageRange::default()).expect("应读取消息");
    assert_eq!(
        messages.iter().map(|message| message.sequence).collect::<Vec<_>>(),
        vec![1, 2]
    );
    match &messages[0].content[0] {
        ContentPart::Text { text } => assert_eq!(text, "first"),
    }

    let page = repo
        .get_messages(
            &session.id,
            MessageRange {
                limit: Some(1),
                after_sequence: Some(1),
            },
        )
        .expect("应读取消息分页");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].sequence, 2);

    let snapshot = repo.resume_session(&session.id).expect("应恢复 Session");
    assert_eq!(snapshot.session.message_count, 2);
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.messages[1].sequence, 2);
}

#[test]
fn list_filters_orders_and_paginates_stably() {
    let root = TestRoot::new();
    let path = root.database_path();
    let mut repo = repository(&path);
    let first = repo.create_session(CreateSession::new("cli")).expect("应创建 Session");
    let second = repo.create_session(CreateSession::new("stdio")).expect("应创建 Session");
    let third = repo.create_session(CreateSession::new("cli")).expect("应创建 Session");

    let cli = repo
        .list_sessions(ListSessions {
            source: Some("cli".to_string()),
            limit: Some(MAX_LIST_LIMIT),
            ..Default::default()
        })
        .expect("应按 source 过滤");
    assert_eq!(cli.len(), 2);
    assert!(cli.iter().all(|summary| summary.source == "cli"));
    assert!(cli[0].updated_at > cli[1].updated_at || cli[0].id.0 < cli[1].id.0);

    let cursor = SessionCursor {
        updated_at: cli[0].updated_at.clone(),
        id: cli[0].id.clone(),
    };
    let next = repo
        .list_sessions(ListSessions {
            source: Some("cli".to_string()),
            before: Some(cursor),
            limit: Some(1),
            ..Default::default()
        })
        .expect("应按 cursor 分页");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].id, cli[1].id);
    assert_ne!(first.id, second.id);
    assert_ne!(second.id, third.id);
}

#[test]
fn close_is_idempotent_and_rejects_append() {
    let root = TestRoot::new();
    let path = root.database_path();
    let mut repo = repository(&path);
    let session = repo.create_session(CreateSession::new("cli")).expect("应创建 Session");
    let closed = repo.close_session(&session.id, Some("finished")).expect("应关闭 Session");
    assert_eq!(closed.status, SessionStatus::Closed);
    assert_eq!(closed.revision, 1);
    let closed_again = repo.close_session(&session.id, None).expect("重复关闭应幂等");
    assert_eq!(closed_again.revision, 1);
    assert_eq!(closed_again.status, SessionStatus::Closed);

    let error = repo
        .append_message(&session.id, AppendMessage::text(Role::User, "after close"))
        .expect_err("关闭 Session 不应接受消息");
    assert!(matches!(error, RepositoryError::SessionClosed(_)));
    let final_state = repo.get_session(&session.id).expect("应读取状态").unwrap();
    assert_eq!(final_state.message_count, 0);
    assert_eq!(final_state.revision, 1);
}

#[test]
fn failed_writes_leave_count_revision_and_messages_unchanged() {
    let root = TestRoot::new();
    let path = root.database_path();
    let mut repo = repository(&path);
    let session = repo.create_session(CreateSession::new("cli")).expect("应创建 Session");
    let missing = SessionId("missing".to_string());
    let error = repo
        .append_message(&missing, AppendMessage::text(Role::User, "no session"))
        .expect_err("不存在的 Session 应失败");
    assert!(matches!(error, RepositoryError::NotFound(_)));
    let state = repo.get_session(&session.id).expect("应读取状态").unwrap();
    assert_eq!(state.message_count, 0);
    assert_eq!(state.revision, 0);
    assert!(repo
        .get_messages(&session.id, MessageRange::default())
        .expect("应读取空消息")
        .is_empty());

    let connection = Connection::open(&path).expect("验证连接应打开");
    connection
        .execute(
            "UPDATE sessions SET message_count = 99 WHERE id = ?1",
            [&session.id.0],
        )
        .expect("应注入损坏计数");
    let error = repo.resume_session(&session.id).expect_err("不一致快照应失败");
    assert!(matches!(error, RepositoryError::InconsistentSnapshot(_)));
}

#[test]
fn concurrent_independent_repositories_append_without_duplicate_sequences() {
    let root = TestRoot::new();
    let path = root.database_path();
    let mut setup = repository(&path);
    let session = setup.create_session(CreateSession::new("concurrent")).expect("应创建 Session");
    drop(setup);

    let mut handles = Vec::new();
    for worker in 0..4 {
        let path = path.clone();
        let session_id = session.id.clone();
        handles.push(thread::spawn(move || {
            let mut repo = repository(&path);
            for index in 0..25 {
                repo.append_message(
                    &session_id,
                    AppendMessage::text(Role::User, format!("worker-{worker}-{index}")),
                )
                .expect("并发追加应成功");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("并发线程不应 panic");
    }

    let repo = repository(&path);
    let messages = repo
        .get_messages(
            &session.id,
            MessageRange {
                limit: Some(100),
                after_sequence: None,
            },
        )
        .expect("应读取并发消息");
    assert_eq!(messages.len(), 100);
    assert_eq!(
        messages.iter().map(|message| message.sequence).collect::<Vec<_>>(),
        (1..=100).collect::<Vec<_>>()
    );
    let state = repo.get_session(&session.id).expect("应读取状态").unwrap();
    assert_eq!(state.message_count, 100);
    assert_eq!(state.revision, 100);
}
