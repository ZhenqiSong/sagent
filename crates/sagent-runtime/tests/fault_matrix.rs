//! P2.3 Provider/Tool 回环故障矩阵。
//!
//! 这些测试只使用本地 Mock Provider、临时 Store 和临时 workspace；每个 fixture 都从
//! submit 到持久化终态走真实 Runtime 路径，避免只测孤立 mock 而漏掉 Actor 的竞争窗口。

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use sagent_agent::{RequestId, UserInput};
use sagent_provider::{
    ModelProvider, OpenAiCompatibleProvider, ProviderError, ProviderEvent, ProviderEventSink,
    ProviderFinish, ProviderRequest, StopReason,
    mock::{MockAction, MockProvider, MockSseChunk, MockSseServer},
};
use sagent_runtime::{RuntimeEventKind, SessionSupervisor, ToolDispatcher, ToolWorker};
use sagent_store::{EventQuery, MessageQuery, NewSession, Store};
use sagent_tools::{
    ReadFileLimits, TerminalLimits, ToolDefinition, ToolPermission, ToolRegistry, WorkspaceRoot,
};
use sagent_types::{EventSequence, SessionId};
use tokio_util::sync::CancellationToken;

fn test_path(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应有效")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "sagent-runtime-fault-{name}-{}-{nonce}.db",
        std::process::id()
    ))
}

fn create_session(path: &Path, id: &SessionId) {
    let mut store = Store::open_readwrite(path).expect("应能打开测试数据库");
    store
        .create_session(&NewSession {
            id: id.clone(),
            source: Some("fault-matrix".into()),
            model: Some("mock".into()),
            title: None,
            started_at: "2026-09-12T00:00:00Z".into(),
        })
        .expect("应能创建测试会话");
}

fn terminal_events(path: &Path, turn_id: &sagent_types::TurnId) -> Vec<String> {
    Store::open_readonly(path)
        .expect("应能读取测试数据库")
        .events_for_turn(turn_id, EventSequence::default())
        .expect("应能读取 Turn 事件")
        .into_iter()
        .filter(|event| {
            matches!(
                event.event_type.as_str(),
                "turn.completed" | "turn.failed" | "turn.interrupted"
            )
        })
        .map(|event| event.event_type)
        .collect()
}

fn tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry
        .register(
            ToolDefinition::new(
                "terminal",
                "执行命令",
                serde_json::json!({
                    "type": "object",
                    "required": ["command"],
                    "properties": {
                        "command": {"type": "string"},
                        "timeout_ms": {"type": "integer", "minimum": 1},
                        "output_limit": {"type": "integer", "minimum": 1}
                    },
                    "additionalProperties": false
                }),
                ToolPermission::ApprovalRequired,
                30_000,
                4_096,
            )
            .expect("terminal schema 应有效"),
        )
        .expect("terminal 应能注册");
    registry
}

#[cfg(windows)]
fn long_running_command() -> &'static str {
    "ping 127.0.0.1 -n 20 > nul"
}

#[cfg(not(windows))]
fn long_running_command() -> &'static str {
    "sleep 20"
}

#[tokio::test]
async fn slow_first_token_keeps_stream_order_and_completes() {
    // Arrange：首个网络 body chunk 故意延迟，验证 Runtime 不把慢响应当作 EOF/失败。
    let path = test_path("slow-first-token");
    let session_id = SessionId::new("fault-slow-first-token");
    create_session(&path, &session_id);
    let server = MockSseServer::spawn(vec![MockSseChunk::delayed(
        include_str!("../../sagent-provider/tests/fixtures/provider/slow_first_token.sse"),
        Duration::from_millis(60),
    )])
    .await
    .expect("应能启动慢首 token fixture");
    let provider = Arc::new(
        OpenAiCompatibleProvider::from_endpoint(&server.url(), "fixture-key")
            .expect("Provider 配置应有效"),
    );
    let factory_path = path.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider, "mock", "fault-v1");
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    let mut events = handle.subscribe();

    // Act：submit 只等待持久化回执，首 token 继续由事件异步送达。
    let receipt = handle
        .submit(
            RequestId::new(),
            UserInput::new("慢首 token").expect("输入有效"),
        )
        .await
        .expect("提交应成功");
    let mut saw_delta = false;
    let mut saw_completed = false;
    for _ in 0..12 {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("慢首 token 不应超时")
            .expect("事件通道应可用");
        match event.kind {
            RuntimeEventKind::ModelTextDelta { ref text } => {
                assert_eq!(text, "慢首 token");
                saw_delta = true;
            }
            RuntimeEventKind::TurnCompleted => {
                saw_completed = true;
                break;
            }
            _ => {}
        }
    }

    // Assert：成功终态必须唯一，且慢首 token 之前仍已提交用户消息。
    assert!(saw_delta);
    assert!(saw_completed);
    assert_eq!(
        terminal_events(&path, &receipt.turn_id),
        vec!["turn.completed"]
    );
    let messages = Store::open_readonly(&path)
        .expect("应能读取消息")
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取 transcript");
    assert_eq!(
        messages.last().map(|message| message.content.as_str()),
        Some("慢首 token")
    );
    handle.close().await.expect("actor 应能关闭");
    server.wait().await.expect("fixture server 应结束");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn repeated_delta_is_transient_and_persists_one_final_message() {
    // Arrange：重复 delta 可能来自上游重试；它们只能进入实时流，不能制造第二个 final。
    let path = test_path("duplicate-delta");
    let session_id = SessionId::new("fault-duplicate-delta");
    create_session(&path, &session_id);
    let provider = Arc::new(MockProvider::new([
        MockAction::Delta("重复".into()),
        MockAction::Delta("重复".into()),
        MockAction::Finish(StopReason::Stop),
    ]));
    let factory_path = path.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider, "mock", "fault-v1");
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    let mut events = handle.subscribe();
    let receipt = handle
        .submit(
            RequestId::new(),
            UserInput::new("重复 delta").expect("输入有效"),
        )
        .await
        .expect("提交应成功");

    // Act：消费到唯一持久化终态。
    let mut delta_count = 0;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("重复 delta 不应阻塞")
            .expect("事件通道应可用");
        match event.kind {
            RuntimeEventKind::ModelTextDelta { .. } => delta_count += 1,
            RuntimeEventKind::TurnCompleted => break,
            _ => {}
        }
    }

    // Assert：两次瞬态 delta 不进入 replay，assistant final 与 terminal event 各一条。
    assert_eq!(delta_count, 2);
    let store = Store::open_readonly(&path).expect("应能读取数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取 transcript");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].content, "重复重复");
    let persisted = store
        .events_since(&EventQuery {
            session_id: session_id.clone(),
            after_sequence: EventSequence::default(),
            limit: 200,
        })
        .expect("应能读取 replay 事件");
    assert!(persisted.iter().all(|event| {
        !matches!(
            event.event_type.as_str(),
            "model.text.delta" | "model.usage"
        )
    }));
    assert_eq!(
        terminal_events(&path, &receipt.turn_id),
        vec!["turn.completed"]
    );
    handle.close().await.expect("actor 应能关闭");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn tool_call_eof_fails_once_without_assistant_tool_call_message() {
    // Arrange：Provider 已经发出 tool-call delta，但连接在 finish/[DONE] 前 EOF。
    let path = test_path("tool-call-eof");
    let session_id = SessionId::new("fault-tool-call-eof");
    create_session(&path, &session_id);
    let server = MockSseServer::spawn(vec![MockSseChunk::text(include_str!(
        "../../sagent-provider/tests/fixtures/provider/tool_call_eof.sse"
    ))])
    .await
    .expect("应能启动 tool-call EOF fixture");
    let provider = Arc::new(
        OpenAiCompatibleProvider::from_endpoint(&server.url(), "fixture-key")
            .expect("Provider 配置应有效"),
    );
    let factory_path = path.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider, "mock", "fault-v1");
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    let mut events = handle.subscribe();
    let receipt = handle
        .submit(
            RequestId::new(),
            UserInput::new("工具 EOF").expect("输入有效"),
        )
        .await
        .expect("提交应成功");

    // Act：等待 ProviderError::IncompleteStream 转成唯一 TurnFailed。
    let failed = loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("EOF 失败应及时收口")
            .expect("事件通道应可用");
        if let RuntimeEventKind::TurnFailed { reason } = event.kind {
            break reason;
        }
    };

    // Assert：未收到完整 tool-call 时不能提交 assistant(tool_calls)，也不能完成 Turn。
    assert!(failed.contains("provider 流未正常结束"));
    assert_eq!(
        terminal_events(&path, &receipt.turn_id),
        vec!["turn.failed"]
    );
    let messages = Store::open_readonly(&path)
        .expect("应能读取数据库")
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取 transcript");
    assert_eq!(messages.len(), 1);
    handle.close().await.expect("actor 应能关闭");
    server.wait().await.expect("fixture server 应结束");
    let _ = fs::remove_file(path);
}

/// 第一次请求超时工具，第二次只在已持久化 tool message 后返回最终文本。
struct TimeoutProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl ModelProvider for TimeoutProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        sink: &mut dyn ProviderEventSink,
        _cancellation: CancellationToken,
    ) -> Result<ProviderFinish, ProviderError> {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                sink.emit(ProviderEvent::ToolCallDelta {
                    call_id: "call_timeout".into(),
                    name: Some("terminal".into()),
                    arguments_delta: serde_json::json!({
                        "command": long_running_command(),
                        "timeout_ms": 50,
                        "output_limit": 1024
                    })
                    .to_string(),
                })
                .await?;
                sink.emit(ProviderEvent::Finished {
                    reason: StopReason::ToolCalls,
                })
                .await?;
                Ok(ProviderFinish {
                    reason: StopReason::ToolCalls,
                    usage: None,
                    provider_request_id: None,
                })
            }
            1 => {
                assert_eq!(request.messages.len(), 4);
                assert_eq!(
                    request.messages[2].role,
                    sagent_provider::ProviderRole::Assistant
                );
                assert_eq!(
                    request.messages[3].tool_call_id.as_deref(),
                    Some("call_timeout")
                );
                assert!(request.messages[3].content.contains("超时"));
                sink.emit(ProviderEvent::TextDelta {
                    text: "工具超时已收到".into(),
                })
                .await?;
                sink.emit(ProviderEvent::Finished {
                    reason: StopReason::Stop,
                })
                .await?;
                Ok(ProviderFinish {
                    reason: StopReason::Stop,
                    usage: None,
                    provider_request_id: None,
                })
            }
            _ => Err(ProviderError::Protocol(
                "超时工具不应重复执行 Provider".into(),
            )),
        }
    }
}

#[tokio::test]
async fn tool_timeout_is_replayed_once_and_does_not_repeat_execution() {
    // Arrange：工具请求显式设置极短 timeout，Provider 第二轮验证回放的失败结果。
    let path = test_path("tool-timeout");
    let session_id = SessionId::new("fault-tool-timeout");
    create_session(&path, &session_id);
    let workspace = std::env::temp_dir().join(format!(
        "sagent-runtime-fault-workspace-{}-{}",
        std::process::id(),
        path.file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("tmp")
    ));
    let _ = fs::remove_dir_all(&workspace);
    fs::create_dir_all(&workspace).expect("应能创建 workspace");
    let provider = Arc::new(TimeoutProvider {
        calls: AtomicUsize::new(0),
    });
    let factory_path = path.clone();
    let worker = ToolWorker::new(
        WorkspaceRoot::new(&workspace).expect("workspace 应有效"),
        ReadFileLimits::default(),
        TerminalLimits::default(),
    );
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider.clone(), "mock", "fault-v1")
    .with_tool_dispatcher(ToolDispatcher::new(tool_registry()))
    .with_tool_worker(worker);
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    let mut events = handle.subscribe();
    let receipt = handle
        .submit(
            RequestId::new(),
            UserInput::new("运行超时命令").expect("输入有效"),
        )
        .await
        .expect("提交应成功");

    // Act：等待工具失败结果和后续最终回答。
    let mut saw_timeout = false;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("工具 timeout 应及时收口")
            .expect("事件通道应可用");
        match event.kind {
            RuntimeEventKind::ToolCompleted {
                ref call_id,
                ok: false,
                ref error_kind,
                ..
            } if call_id == "call_timeout" => {
                saw_timeout = error_kind.as_deref() == Some("timeout");
            }
            RuntimeEventKind::TurnCompleted => break,
            _ => {}
        }
    }

    // Assert：timeout 是一个可回放的 tool message；同一调用只执行一次且终态唯一。
    assert!(saw_timeout);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        terminal_events(&path, &receipt.turn_id),
        vec!["turn.completed"]
    );
    let messages = Store::open_readonly(&path)
        .expect("应能读取数据库")
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取 transcript");
    assert_eq!(messages.len(), 4);
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.tool_call_id.as_deref() == Some("call_timeout"))
            .count(),
        1
    );
    handle.close().await.expect("actor 应能关闭");
    let _ = fs::remove_file(path);
    let _ = fs::remove_dir_all(workspace);
}
