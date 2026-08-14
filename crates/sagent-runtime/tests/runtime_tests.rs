//! Runtime Supervisor 真实 SQLite 生命周期和恢复集成测试。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 6 Runtime 测试

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sagent_config::{Config, ConfigPaths};
use sagent_runtime::{Runtime, RuntimeError, SessionView};
use sagent_session::{AppendMessage, CreateSession, ListSessions};
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
            "sagent-runtime-supervisor-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("应创建测试目录");
        Self(path)
    }

    fn paths(&self) -> ConfigPaths {
        ConfigPaths::from_root(self.0.clone())
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn config() -> Config {
    Config::default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_opens_database_before_accepting_session_commands() {
    let root = TestRoot::new();
    let runtime = Runtime::open_at(config(), root.paths()).expect("Runtime 应成功打开");
    assert!(runtime.database_path().exists());
    let handle = runtime
        .create_session(CreateSession::new("runtime"))
        .await
        .expect("Runtime 应创建 Session");
    assert_eq!(
        handle.snapshot().await.expect("应读取快照").session.message_count,
        0
    );
    runtime.shutdown().await.expect("Runtime 应关闭");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_lists_gets_and_resumes_committed_session() {
    let root = TestRoot::new();
    let runtime = Runtime::open_at(config(), root.paths()).expect("Runtime 应打开");
    let handle = runtime
        .create_session(CreateSession::new("restart"))
        .await
        .expect("Session 应创建");
    let session_id = handle.session_id().clone();
    handle
        .append_message(AppendMessage::text(Role::User, "committed"))
        .await
        .expect("消息应提交");
    runtime.shutdown().await.expect("第一次 Runtime 应关闭");

    let restarted = Runtime::open_at(config(), root.paths()).expect("Runtime 应重启");
    let listed = restarted.list_sessions(ListSessions::default()).await.expect("应列出 Session");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, session_id);

    let view = restarted
        .get_session(&session_id)
        .await
        .expect("get 应成功")
        .expect("Session 应存在");
    match view {
        SessionView::Snapshot(snapshot) => {
            assert_eq!(snapshot.session.message_count, 1);
            assert_eq!(snapshot.messages[0].sequence, 1);
        },
        SessionView::Live(_) => panic!("重启后 Session 不应隐式成为 live Actor"),
    }

    let resumed = restarted.resume_session(&session_id).await.expect("resume 应成功");
    assert_eq!(
        resumed.snapshot().await.expect("应读取恢复快照").messages.len(),
        1
    );
    let resumed_again = restarted.resume_session(&session_id).await.expect("重复 resume 应成功");
    assert_eq!(resumed.session_id(), resumed_again.session_id());
    restarted.shutdown().await.expect("第二次 Runtime 应关闭");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn corrupt_session_fails_closed_without_blocking_list() {
    let root = TestRoot::new();
    let runtime = Runtime::open_at(config(), root.paths()).expect("Runtime 应打开");
    let handle = runtime
        .create_session(CreateSession::new("healthy"))
        .await
        .expect("Session 应创建");
    let healthy_id = handle.session_id().clone();
    handle
        .append_message(AppendMessage::text(Role::User, "message"))
        .await
        .expect("消息应提交");
    runtime.shutdown().await.expect("Runtime 应关闭");

    let database_path = root.0.join("state.db");
    let connection = rusqlite::Connection::open(&database_path).expect("注入连接应打开");
    connection
        .execute(
            "UPDATE sessions SET message_count = 99 WHERE id = ?1",
            [&healthy_id.0],
        )
        .expect("应注入损坏计数");
    drop(connection);

    let restarted = Runtime::open_at(config(), root.paths()).expect("损坏数据不应阻止数据库启动");
    let list = restarted
        .list_sessions(ListSessions::default())
        .await
        .expect("list 不应被单个损坏 Session 阻塞");
    assert_eq!(list.len(), 1);
    let error = restarted
        .resume_session(&healthy_id)
        .await
        .expect_err("损坏 transcript 必须 fail closed");
    assert!(matches!(
        error,
        RuntimeError::Repository(sagent_session::RepositoryError::InconsistentSnapshot(_))
    ));
    restarted.shutdown().await.expect("Runtime 应关闭");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_is_idempotent_and_rejects_new_sessions() {
    let root = TestRoot::new();
    let runtime = Runtime::open_at(config(), root.paths()).expect("Runtime 应打开");
    runtime.shutdown().await.expect("第一次 shutdown 应成功");
    runtime.shutdown().await.expect("重复 shutdown 应幂等");
    let error = runtime
        .create_session(CreateSession::new("after-shutdown"))
        .await
        .expect_err("shutdown 后不应创建 Session");
    assert!(matches!(error, RuntimeError::ShuttingDown));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn max_live_session_limit_is_enforced_without_losing_persisted_rows() {
    let root = TestRoot::new();
    let mut config = config();
    config.runtime.max_live_sessions = 1;
    let runtime = Runtime::open_at(config, root.paths()).expect("Runtime 应打开");
    runtime
        .create_session(CreateSession::new("first"))
        .await
        .expect("第一个 Session 应创建");
    let error = runtime
        .create_session(CreateSession::new("second"))
        .await
        .expect_err("第二个 live Session 应被限制");
    assert!(matches!(error, RuntimeError::MaxLiveSessions));
    let listed = runtime.list_sessions(ListSessions::default()).await.expect("list 应成功");
    assert_eq!(listed.len(), 1);
    runtime.shutdown().await.expect("Runtime 应关闭");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_does_not_leave_actor_tasks_running() {
    let root = TestRoot::new();
    let runtime = Runtime::open_at(config(), root.paths()).expect("Runtime 应打开");
    let handle = runtime
        .create_session(CreateSession::new("shutdown"))
        .await
        .expect("Session 应创建");
    runtime.shutdown().await.expect("Runtime 应关闭");
    let result = timeout(Duration::from_millis(100), handle.snapshot()).await;
    assert!(result.is_ok());
    assert!(matches!(
        result.expect("timeout 已完成"),
        Err(sagent_runtime::ActorError::Shutdown(_))
    ));
}
