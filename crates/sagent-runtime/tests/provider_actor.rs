use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use sagent_agent::{RequestId, UserInput};
use sagent_provider::mock::{MockAction, MockProvider, MockSseChunk, MockSseServer};
use sagent_provider::{OpenAiCompatibleProvider, ProviderError, StopReason};
use sagent_runtime::{RuntimeError, RuntimeEventKind, SessionSupervisor};
use sagent_store::{MessageQuery, NewSession, Store};
use sagent_types::{EventSequence, SessionId};

fn test_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sagent-runtime-provider-{name}-{}.db",
        std::process::id()
    ))
}

fn create_session(path: &Path, id: &SessionId) {
    let mut store = Store::open_readwrite(path).expect("应能打开测试数据库");
    store
        .create_session(&NewSession {
            id: id.clone(),
            source: Some("provider-integration-test".into()),
            model: Some("test-model".into()),
            title: None,
            started_at: "2026-09-05T00:00:00Z".into(),
        })
        .expect("应能创建测试会话");
}

#[tokio::test]
async fn provider_events_are_bridged_and_final_text_is_persisted() {
    let path = test_path("normal");
    let _ = fs::remove_file(&path);
    let session_id = SessionId::new("provider-session");
    create_session(&path, &session_id);
    let factory_path = path.clone();
    let provider = Arc::new(MockProvider::new([
        MockAction::Delta("你好".into()),
        MockAction::Delta("，Provider".into()),
        MockAction::Finish(StopReason::Stop),
    ]));
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider, "test-model", "profile-v1");
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    let mut events = handle.subscribe();

    handle
        .submit(
            RequestId::new(),
            UserInput::new("请回答").expect("输入有效"),
        )
        .await
        .expect("提交应成功");

    assert!(matches!(
        events.recv().await.expect("事件通道应可用").kind,
        RuntimeEventKind::PromptAccepted
    ));
    assert!(matches!(
        events.recv().await.expect("事件通道应可用").kind,
        RuntimeEventKind::UserMessagePersisted { .. }
    ));
    assert!(matches!(
        events.recv().await.expect("事件通道应可用").kind,
        RuntimeEventKind::ModelTextDelta { ref text } if text == "你好"
    ));
    assert!(matches!(
        events.recv().await.expect("事件通道应可用").kind,
        RuntimeEventKind::ModelTextDelta { ref text } if text == "，Provider"
    ));
    assert!(matches!(
        events.recv().await.expect("事件通道应可用").kind,
        RuntimeEventKind::FinalMessagePersisted { .. }
    ));
    assert!(matches!(
        events.recv().await.expect("事件通道应可用").kind,
        RuntimeEventKind::TurnCompleted
    ));

    let store = Store::open_readonly(&path).expect("应能读取数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].content, "你好，Provider");
    let generation = store
        .get_generation(&session_id, 0)
        .expect("应能读取 generation")
        .expect("generation 应存在");
    assert_eq!(generation.model_id, "test-model");
    assert_eq!(generation.profile_revision, "profile-v1");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn openai_sse_is_driven_through_provider_worker_and_actor() {
    let path = test_path("sse");
    let _ = fs::remove_file(&path);
    let session_id = SessionId::new("provider-sse-session");
    create_session(&path, &session_id);
    let server = MockSseServer::spawn(vec![MockSseChunk::text(include_str!(
        "../../sagent-provider/tests/fixtures/provider/normal_text.sse"
    ))])
    .await
    .expect("应能启动 SSE server");
    let provider = Arc::new(
        OpenAiCompatibleProvider::from_endpoint(&server.url(), "test-key")
            .expect("Provider 配置有效"),
    );
    let factory_path = path.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider, "sse-model", "sse-profile");
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    let mut events = handle.subscribe();

    handle
        .submit(
            RequestId::new(),
            UserInput::new("读取 SSE").expect("输入有效"),
        )
        .await
        .expect("提交应成功");

    let mut deltas = Vec::new();
    let mut completed = false;
    for _ in 0..8 {
        let event = events.recv().await.expect("事件通道应可用");
        match event.kind {
            RuntimeEventKind::ModelTextDelta { text } => deltas.push(text),
            RuntimeEventKind::TurnCompleted => {
                completed = true;
                break;
            }
            _ => {}
        }
    }
    assert_eq!(deltas.concat(), "你好，Sagent");
    assert!(completed);
    server.wait().await.expect("SSE server 应完成");

    let store = Store::open_readonly(&path).expect("应能打开数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(
        messages.last().map(|message| message.content.as_str()),
        Some("你好，Sagent")
    );
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn provider_usage_is_published_without_polluting_assistant_message() {
    let path = test_path("usage");
    let _ = fs::remove_file(&path);
    let session_id = SessionId::new("provider-usage-session");
    create_session(&path, &session_id);
    let server = MockSseServer::spawn(vec![MockSseChunk::text(include_str!(
        "../../sagent-provider/tests/fixtures/provider/usage.sse"
    ))])
    .await
    .expect("应能启动 SSE server");
    let provider = Arc::new(
        OpenAiCompatibleProvider::from_endpoint(&server.url(), "test-key")
            .expect("Provider 配置有效"),
    );
    let factory_path = path.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider, "usage-model", "usage-profile");
    let handle = supervisor.get_or_start(session_id.clone()).await.unwrap();
    let mut events = handle.subscribe();
    handle
        .submit(RequestId::new(), UserInput::new("统计用量").unwrap())
        .await
        .unwrap();

    let mut usage = None;
    let mut completed = false;
    for _ in 0..10 {
        let event = events.recv().await.unwrap();
        match event.kind {
            RuntimeEventKind::ModelUsage { usage: value } => usage = Some(value),
            RuntimeEventKind::TurnCompleted => {
                completed = true;
                break;
            }
            _ => {}
        }
    }
    assert!(completed);
    let usage = usage.expect("应发布模型用量事件");
    assert_eq!(usage.prompt_tokens, 10);
    assert_eq!(usage.completion_tokens, 2);
    assert_eq!(usage.total_tokens, 12);
    server.wait().await.expect("SSE server 应完成");

    let store = Store::open_readonly(&path).unwrap();
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .unwrap();
    assert_eq!(
        messages.last().map(|message| message.content.as_str()),
        Some("完成")
    );
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn provider_failure_does_not_create_assistant_message() {
    let path = test_path("failure");
    let _ = fs::remove_file(&path);
    let session_id = SessionId::new("provider-failure-session");
    create_session(&path, &session_id);
    let provider = Arc::new(MockProvider::new([MockAction::Fail(
        ProviderError::Authentication,
    )]));
    let factory_path = path.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider, "test-model", "profile-v1");
    let handle = supervisor.get_or_start(session_id.clone()).await.unwrap();
    let mut events = handle.subscribe();
    handle
        .submit(RequestId::new(), UserInput::new("失败").unwrap())
        .await
        .unwrap();

    let mut failed = false;
    for _ in 0..6 {
        let event = events.recv().await.unwrap();
        if matches!(event.kind, RuntimeEventKind::TurnFailed { .. }) {
            failed = true;
            break;
        }
    }
    assert!(failed);
    let store = Store::open_readonly(&path).unwrap();
    assert_eq!(
        store
            .get_messages_for_display(&session_id, &MessageQuery::default())
            .unwrap()
            .len(),
        1
    );
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn cancellation_during_provider_worker_does_not_create_assistant_message() {
    let path = test_path("cancel");
    let _ = fs::remove_file(&path);
    let session_id = SessionId::new("provider-cancel-session");
    create_session(&path, &session_id);
    let provider = Arc::new(MockProvider::new([MockAction::WaitForCancel]));
    let factory_path = path.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider, "test-model", "profile-v1");
    let handle = supervisor.get_or_start(session_id.clone()).await.unwrap();
    let mut events = handle.subscribe();
    handle
        .submit(RequestId::new(), UserInput::new("取消").unwrap())
        .await
        .unwrap();
    let _ = events.recv().await;
    let _ = events.recv().await;

    handle.interrupt(RequestId::new()).await.unwrap();
    let interrupted = events.recv().await.unwrap();
    assert!(matches!(
        interrupted.kind,
        RuntimeEventKind::TurnInterrupted
    ));
    let store = Store::open_readonly(&path).unwrap();
    assert_eq!(
        store
            .get_messages_for_display(&session_id, &MessageQuery::default())
            .unwrap()
            .len(),
        1
    );
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn late_interrupt_cannot_overwrite_a_completed_turn() {
    let path = test_path("late-interrupt");
    let _ = fs::remove_file(&path);
    let session_id = SessionId::new("provider-late-interrupt-session");
    create_session(&path, &session_id);
    let provider = Arc::new(MockProvider::new([MockAction::Finish(StopReason::Stop)]));
    let factory_path = path.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider, "test-model", "profile-v1");
    let handle = supervisor.get_or_start(session_id.clone()).await.unwrap();
    let mut events = handle.subscribe();
    let receipt = handle
        .submit(RequestId::new(), UserInput::new("立即完成").unwrap())
        .await
        .unwrap();

    loop {
        let event = events.recv().await.unwrap();
        if matches!(event.kind, RuntimeEventKind::TurnCompleted) {
            break;
        }
    }
    assert!(matches!(
        handle.interrupt(RequestId::new()).await,
        Err(RuntimeError::NoActiveTurn)
    ));

    let store = Store::open_readonly(&path).unwrap();
    let terminal_events = store
        .events_for_turn(&receipt.turn_id, EventSequence::default())
        .unwrap()
        .into_iter()
        .filter(|event| {
            matches!(
                event.event_type.as_str(),
                "turn.completed" | "turn.failed" | "turn.interrupted"
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 1);
    assert_eq!(terminal_events[0].event_type, "turn.completed");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn incomplete_sse_stream_fails_turn_without_empty_assistant_message() {
    let path = test_path("eof");
    let _ = fs::remove_file(&path);
    let session_id = SessionId::new("provider-eof-session");
    create_session(&path, &session_id);
    let server = MockSseServer::spawn(vec![MockSseChunk::text(include_str!(
        "../../sagent-provider/tests/fixtures/provider/eof_without_finish.sse"
    ))])
    .await
    .expect("应能启动 SSE server");
    let provider = Arc::new(
        OpenAiCompatibleProvider::from_endpoint(&server.url(), "test-key")
            .expect("Provider 配置有效"),
    );
    let factory_path = path.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider, "eof-model", "profile-v1");
    let handle = supervisor.get_or_start(session_id.clone()).await.unwrap();
    let mut events = handle.subscribe();
    handle
        .submit(RequestId::new(), UserInput::new("EOF").unwrap())
        .await
        .unwrap();

    let mut failed = false;
    for _ in 0..8 {
        let event = events.recv().await.unwrap();
        if matches!(event.kind, RuntimeEventKind::TurnFailed { .. }) {
            failed = true;
            break;
        }
    }
    assert!(failed);
    server.wait().await.expect("SSE server 应完成");
    let store = Store::open_readonly(&path).unwrap();
    assert_eq!(
        store
            .get_messages_for_display(&session_id, &MessageQuery::default())
            .unwrap()
            .len(),
        1
    );
    let _ = fs::remove_file(path);
}
