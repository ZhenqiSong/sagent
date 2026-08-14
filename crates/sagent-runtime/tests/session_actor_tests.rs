//! Session Actor 真实 SQLite 和 Tokio 集成测试。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 5 Actor 测试

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sagent_config::{DatabaseConfig, SynchronousMode};
use sagent_runtime::{SessionActor, SessionEvent};
use sagent_session::{AppendMessage, CreateSession, DatabaseConnection, MessageRange, Repository};
use sagent_types::message::Role;
use tokio::time::{timeout, Duration};

static TEST_ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let timestamp =
            SystemTime::now().duration_since(UNIX_EPOCH).expect("系统时间应有效").as_nanos();
        let sequence = TEST_ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sagent-runtime-{}-{timestamp}-{sequence}",
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

fn create_actor(
    path: &PathBuf,
    mailbox_capacity: usize,
) -> (sagent_runtime::SessionHandle, tokio::task::JoinHandle<()>) {
    let mut setup =
        Repository::new(DatabaseConnection::open(path, &config()).expect("数据库应打开"));
    let session = setup
        .create_session(CreateSession::new("runtime-test"))
        .expect("Session 应创建");
    let snapshot = setup.resume_session(&session.id).expect("Session 应恢复");
    drop(setup);
    let database = DatabaseConnection::open(path, &config()).expect("Actor 数据库应打开");
    SessionActor::spawn(database, snapshot.into(), mailbox_capacity, 32)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_session_commands_are_serialized_in_sequence_order() {
    let root = TestRoot::new();
    let (handle, task) = create_actor(&root.database_path(), 256);
    let mut calls = Vec::new();
    for index in 0..100 {
        let handle = handle.clone();
        calls.push(tokio::spawn(async move {
            handle
                .append_message(AppendMessage::text(Role::User, format!("message-{index}")))
                .await
        }));
    }

    let mut sequences = Vec::new();
    for call in calls {
        sequences
            .push(call.await.expect("append task 不应 panic").expect("append 应成功").sequence);
    }
    sequences.sort_unstable();
    assert_eq!(sequences, (1..=100).collect::<Vec<_>>());

    let snapshot = handle.snapshot().await.expect("应读取 Actor 快照");
    assert_eq!(snapshot.session.message_count, 100);
    assert_eq!(snapshot.session.revision, 100);
    assert_eq!(snapshot.messages.len(), 100);

    handle.shutdown().await.expect("Actor 应关闭");
    task.await.expect("Actor task 不应 panic");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn different_sessions_have_independent_sequences() {
    let root = TestRoot::new();
    let (first, first_task) = create_actor(&root.database_path(), 256);
    let (second, second_task) = create_actor(&root.database_path(), 256);

    let mut calls = Vec::new();
    for handle in [first.clone(), second.clone()] {
        for index in 0..100 {
            let handle = handle.clone();
            calls.push(tokio::spawn(async move {
                handle
                    .append_message(AppendMessage::text(Role::User, format!("message-{index}")))
                    .await
            }));
        }
    }
    for call in calls {
        call.await.expect("append task 不应 panic").expect("append 应成功");
    }

    for handle in [first.clone(), second.clone()] {
        let messages = handle
            .list_messages(MessageRange {
                limit: Some(100),
                ..Default::default()
            })
            .await
            .expect("应读取消息");
        assert_eq!(messages.len(), 100);
        assert_eq!(messages[0].sequence, 1);
        assert_eq!(messages[99].sequence, 100);
        handle.shutdown().await.expect("Actor 应关闭");
    }
    first_task.await.expect("第一个 Actor 不应 panic");
    second_task.await.expect("第二个 Actor 不应 panic");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_mailbox_returns_explicit_error() {
    let root = TestRoot::new();
    let (handle, task) = create_actor(&root.database_path(), 1);
    let mut calls = Vec::new();
    for index in 0..1_000 {
        let handle = handle.clone();
        calls.push(tokio::spawn(async move {
            handle.append_message(AppendMessage::text(Role::User, index.to_string())).await
        }));
    }

    let mut mailbox_full = false;
    for call in calls {
        if matches!(
            call.await.expect("append task 不应 panic"),
            Err(sagent_runtime::ActorError::MailboxFull(_))
        ) {
            mailbox_full = true;
        }
    }
    assert!(mailbox_full, "mailbox 压满时必须返回 MailboxFull");
    handle.shutdown().await.expect("Actor 应关闭");
    task.await.expect("Actor task 不应 panic");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn events_are_published_after_commit_and_disconnected_subscribers_do_not_block() {
    let root = TestRoot::new();
    let (handle, task) = create_actor(&root.database_path(), 64);
    let disconnected = handle.subscribe().await.expect("应创建订阅");
    drop(disconnected);
    let mut events = handle.subscribe().await.expect("应创建第二个订阅");

    let message = handle
        .append_message(AppendMessage::text(Role::User, "committed"))
        .await
        .expect("append 应成功");
    let event = timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("应在超时前收到事件")
        .expect("订阅不应提前关闭");
    match event {
        SessionEvent::MessageAppended {
            message: event_message,
            revision,
            seq,
        } => {
            assert_eq!(event_message.message_id, message.message_id);
            assert_eq!(revision, 1);
            assert_eq!(seq, 1);
        },
        other => panic!("收到错误事件: {other:?}"),
    }

    handle.close(Some("done".to_string())).await.expect("应关闭 Session");
    assert!(matches!(
        timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("应收到 close event")
            .expect("订阅不应关闭"),
        SessionEvent::Closed { seq: 2, .. }
    ));
    let error = handle
        .append_message(AppendMessage::text(Role::User, "not-committed"))
        .await
        .expect_err("关闭后 append 应失败");
    assert!(matches!(
        error,
        sagent_runtime::ActorError::Repository(sagent_session::RepositoryError::SessionClosed(_))
    ));
    assert!(timeout(Duration::from_millis(50), events.recv()).await.is_err());

    handle.shutdown().await.expect("Actor 应关闭");
    task.await.expect("Actor task 不应 panic");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_rejects_new_commands() {
    let root = TestRoot::new();
    let (handle, task) = create_actor(&root.database_path(), 8);
    handle.shutdown().await.expect("Actor 应关闭");
    task.await.expect("Actor task 不应 panic");

    let error = handle.snapshot().await.expect_err("shutdown 后不应接受新命令");
    assert!(matches!(error, sagent_runtime::ActorError::Shutdown(_)));
}
