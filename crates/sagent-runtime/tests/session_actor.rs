use std::fs;
use std::path::{Path, PathBuf};

use sagent_agent::RequestId;
use sagent_runtime::{RuntimeError, RuntimeEventKind, SessionSupervisor};
use sagent_store::{MessageQuery, NewSession, Store};
use sagent_types::SessionId;

fn test_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sagent-runtime-integration-{name}-{}.db",
        std::process::id()
    ))
}

fn create_session(path: &Path, id: &SessionId) {
    let mut store = Store::open_readwrite(path).expect("应能打开测试数据库");
    store
        .create_session(&NewSession {
            id: id.clone(),
            source: Some("integration-test".into()),
            model: Some("test-model".into()),
            title: None,
            started_at: "2026-09-04T00:00:00Z".into(),
        })
        .expect("应能创建测试会话");
}

#[tokio::test]
async fn public_handle_serializes_submit_and_persists_interrupt() {
    let path = test_path("serial-submit");
    let _ = fs::remove_file(&path);
    let session_id = SessionId::new("integration-serial");
    create_session(&path, &session_id);
    let factory_path = path.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    });
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    let mut events = handle.subscribe();

    let _accepted = handle
        .submit(
            RequestId::new(),
            sagent_agent::UserInput::new("第一条").expect("输入有效"),
        )
        .await
        .expect("首条消息应被接受");
    assert!(matches!(
        events.recv().await.expect("事件通道应可用").kind,
        RuntimeEventKind::PromptAccepted
    ));
    assert!(matches!(
        events.recv().await.expect("事件通道应可用").kind,
        RuntimeEventKind::UserMessagePersisted { .. }
    ));

    let busy = handle
        .submit(
            RequestId::new(),
            sagent_agent::UserInput::new("第二条").expect("输入有效"),
        )
        .await;
    assert!(matches!(busy, Err(RuntimeError::Busy { .. })));

    handle
        .interrupt(RequestId::new())
        .await
        .expect("中断应成功");
    assert!(matches!(
        events.recv().await.expect("事件通道应可用").kind,
        RuntimeEventKind::TurnInterrupted
    ));

    let store = Store::open_readonly(&path).expect("应能重新打开数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "第一条");

    handle.close().await.expect("空闲关闭应幂等成功");
    supervisor
        .remove(&session_id)
        .await
        .expect("remove 应幂等成功");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn different_sessions_are_isolated_through_public_supervisor() {
    let path = test_path("isolation");
    let _ = fs::remove_file(&path);
    let session_a = SessionId::new("integration-a");
    let session_b = SessionId::new("integration-b");
    create_session(&path, &session_a);
    create_session(&path, &session_b);
    let factory_path = path.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    });
    let handle_a = supervisor
        .get_or_start(session_a.clone())
        .await
        .expect("应能启动会话 A");
    let handle_b = supervisor
        .get_or_start(session_b.clone())
        .await
        .expect("应能启动会话 B");

    let (a, b) = tokio::join!(
        handle_a.submit(
            RequestId::new(),
            sagent_agent::UserInput::new("A 的消息").expect("输入有效"),
        ),
        handle_b.submit(
            RequestId::new(),
            sagent_agent::UserInput::new("B 的消息").expect("输入有效"),
        )
    );
    assert!(a.is_ok());
    assert!(b.is_ok());

    let store = Store::open_readonly(&path).expect("应能读取数据库");
    let messages_a = store
        .get_messages_for_display(&session_a, &MessageQuery::default())
        .expect("应能读取 A");
    let messages_b = store
        .get_messages_for_display(&session_b, &MessageQuery::default())
        .expect("应能读取 B");
    assert_eq!(messages_a.len(), 1);
    assert_eq!(messages_b.len(), 1);
    assert_eq!(messages_a[0].content, "A 的消息");
    assert_eq!(messages_b[0].content, "B 的消息");

    supervisor.remove(&session_a).await.expect("移除 A 应成功");
    supervisor.remove(&session_b).await.expect("移除 B 应成功");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn closed_handle_is_stale_and_session_can_restart() {
    let path = test_path("restart");
    let _ = fs::remove_file(&path);
    let session_id = SessionId::new("integration-restart");
    create_session(&path, &session_id);
    let factory_path = path.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    });
    let old_handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    old_handle.close().await.expect("关闭应成功");
    let stale = old_handle
        .submit(
            RequestId::new(),
            sagent_agent::UserInput::new("过期消息").expect("输入有效"),
        )
        .await;
    assert!(matches!(stale, Err(RuntimeError::ActorStopped)));

    let new_handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动新 actor");
    new_handle
        .submit(
            RequestId::new(),
            sagent_agent::UserInput::new("重启后的消息").expect("输入有效"),
        )
        .await
        .expect("新 actor 应能接受消息");
    supervisor.remove(&session_id).await.expect("remove 应成功");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn store_open_failure_is_exposed_without_leaking_sqlite_error_type() {
    let supervisor = SessionSupervisor::new(|| Err("测试数据库不可用".to_owned()));
    let result = supervisor
        .get_or_start(SessionId::new("integration-error"))
        .await;
    assert!(matches!(
        result,
        Err(RuntimeError::Persistence(message)) if message.contains("不可用")
    ));
}

#[tokio::test]
async fn separate_profile_databases_do_not_share_actor_data() {
    let path_a = test_path("profile-a");
    let path_b = test_path("profile-b");
    let _ = fs::remove_file(&path_a);
    let _ = fs::remove_file(&path_b);
    let session_id = SessionId::new("same-session-id");
    create_session(&path_a, &session_id);
    create_session(&path_b, &session_id);

    let factory_a = path_a.clone();
    let factory_b = path_b.clone();
    let supervisor_a = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_a).map_err(|error| error.to_string())
    });
    let supervisor_b = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_b).map_err(|error| error.to_string())
    });
    let handle_a = supervisor_a
        .get_or_start(session_id.clone())
        .await
        .expect("profile A 应能启动");
    let handle_b = supervisor_b
        .get_or_start(session_id.clone())
        .await
        .expect("profile B 应能启动");
    handle_a
        .submit(
            RequestId::new(),
            sagent_agent::UserInput::new("来自 A").expect("输入有效"),
        )
        .await
        .expect("A 应能提交");
    handle_b
        .submit(
            RequestId::new(),
            sagent_agent::UserInput::new("来自 B").expect("输入有效"),
        )
        .await
        .expect("B 应能提交");

    let messages_a = Store::open_readonly(&path_a)
        .expect("应能读取 profile A")
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取 A 消息");
    let messages_b = Store::open_readonly(&path_b)
        .expect("应能读取 profile B")
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取 B 消息");
    assert_eq!(messages_a[0].content, "来自 A");
    assert_eq!(messages_b[0].content, "来自 B");

    supervisor_a
        .remove(&session_id)
        .await
        .expect("A remove 应成功");
    supervisor_b
        .remove(&session_id)
        .await
        .expect("B remove 应成功");
    let _ = fs::remove_file(path_a);
    let _ = fs::remove_file(path_b);
}
