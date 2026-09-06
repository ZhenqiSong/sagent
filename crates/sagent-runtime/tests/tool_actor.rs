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
use sagent_agent::{ApprovalDecision, RequestId, UserInput};
use sagent_provider::{
    ModelProvider, ProviderError, ProviderEvent, ProviderEventSink, ProviderFinish,
    ProviderRequest, ProviderRole, StopReason,
};
use sagent_runtime::{RuntimeEventKind, SessionSupervisor, ToolDispatcher, ToolWorker};
use sagent_store::{MessageQuery, NewSession, Store};
use sagent_tools::{
    ReadFileLimits, TerminalLimits, ToolDefinition, ToolPermission, ToolRegistry, WorkspaceRoot,
};
use sagent_types::SessionId;
use tokio_util::sync::CancellationToken;

fn test_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sagent-runtime-tool-actor-{name}-{}.db",
        std::process::id()
    ))
}

fn create_session(path: &Path, id: &SessionId) {
    let mut store = Store::open_readwrite(path).expect("应能打开测试数据库");
    store
        .create_session(&NewSession {
            id: id.clone(),
            source: Some("tool-actor-test".into()),
            model: Some("mock".into()),
            title: None,
            started_at: "2026-09-05T00:00:00Z".into(),
        })
        .expect("应能创建测试会话");
}

fn tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry
        .register(
            ToolDefinition::new(
                "read_file",
                "读取文件",
                serde_json::json!({"type": "object"}),
                ToolPermission::ReadOnly,
                30_000,
                32_768,
            )
            .unwrap(),
        )
        .unwrap();
    registry
        .register(
            ToolDefinition::new(
                "terminal",
                "执行命令",
                serde_json::json!({"type": "object"}),
                ToolPermission::ApprovalRequired,
                30_000,
                32_768,
            )
            .unwrap(),
        )
        .unwrap();
    registry
}

/// 第一轮发起 read_file，第二轮只接受已回放的 tool message 并返回最终答案。
struct TwoRoundProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl ModelProvider for TwoRoundProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        sink: &mut dyn ProviderEventSink,
        _cancellation: CancellationToken,
    ) -> Result<ProviderFinish, ProviderError> {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                assert_eq!(request.tools[0]["function"]["name"], "read_file");
                sink.emit(ProviderEvent::ToolCallDelta {
                    call_id: "call_read_1".into(),
                    name: Some("read_file".into()),
                    arguments_delta: r#"{"path":"notes.txt","offset":1,"limit":1}"#.into(),
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
                assert!(matches!(request.messages[2].role, ProviderRole::Assistant));
                assert_eq!(request.messages[2].tool_calls[0].id, "call_read_1");
                assert_eq!(request.messages[2].tool_calls[0].name, "read_file");
                assert!(matches!(request.messages[3].role, ProviderRole::Tool));
                assert_eq!(
                    request.messages[3].tool_call_id.as_deref(),
                    Some("call_read_1")
                );
                assert_eq!(request.messages[3].content, "1|第一行");
                sink.emit(ProviderEvent::TextDelta {
                    text: "文件第一行是：第一行".into(),
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
            _ => Err(ProviderError::Protocol("不应启动第三轮 Provider".into())),
        }
    }
}

/// 每一轮都请求同一个只读工具，用于确认 Actor 会在设定上限处停止回环。
struct LoopingToolProvider {
    calls: AtomicUsize,
}

/// 第一轮请求一个需要审批、但作用于不存在目标的 terminal 命令；第二轮确认 tool
/// result 已被回放后结束。命令在不同平台都至多返回非零退出，不会删除测试文件。
struct ApprovalProvider {
    calls: AtomicUsize,
}

/// 请求一个会持续一段时间的安全 terminal 命令，用来验证 Turn interrupt 会取消
/// ToolWorker，而迟到的结果不会写入 Store。
struct LongTerminalProvider;

#[async_trait]
impl ModelProvider for LongTerminalProvider {
    async fn stream(
        &self,
        _request: ProviderRequest,
        sink: &mut dyn ProviderEventSink,
        _cancellation: CancellationToken,
    ) -> Result<ProviderFinish, ProviderError> {
        sink.emit(ProviderEvent::ToolCallDelta {
            call_id: "call_long_terminal".into(),
            name: Some("terminal".into()),
            arguments_delta: serde_json::json!({"command": long_running_command()}).to_string(),
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
}

#[cfg(windows)]
fn long_running_command() -> &'static str {
    "ping 127.0.0.1 -n 20 > nul"
}

#[cfg(not(windows))]
fn long_running_command() -> &'static str {
    "sleep 20"
}

#[async_trait]
impl ModelProvider for ApprovalProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        sink: &mut dyn ProviderEventSink,
        _cancellation: CancellationToken,
    ) -> Result<ProviderFinish, ProviderError> {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                assert!(
                    request
                        .tools
                        .iter()
                        .any(|tool| tool["function"]["name"] == "terminal")
                );
                sink.emit(ProviderEvent::ToolCallDelta {
                    call_id: "call_terminal_approval".into(),
                    name: Some("terminal".into()),
                    arguments_delta: r#"{"command":"rm -rf __sagent_approval_missing__"}"#.into(),
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
                    request.messages[2].tool_calls[0].id,
                    "call_terminal_approval"
                );
                assert!(matches!(request.messages[3].role, ProviderRole::Tool));
                assert_eq!(
                    request.messages[3].tool_call_id.as_deref(),
                    Some("call_terminal_approval")
                );
                sink.emit(ProviderEvent::TextDelta {
                    text: "审批后的工具结果已收到".into(),
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
                "审批路径不应启动第三轮 Provider".into(),
            )),
        }
    }
}

#[async_trait]
impl ModelProvider for LoopingToolProvider {
    async fn stream(
        &self,
        _request: ProviderRequest,
        sink: &mut dyn ProviderEventSink,
        _cancellation: CancellationToken,
    ) -> Result<ProviderFinish, ProviderError> {
        let round = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        sink.emit(ProviderEvent::ToolCallDelta {
            call_id: format!("call_loop_{round}"),
            name: Some("read_file".into()),
            arguments_delta: r#"{"path":"notes.txt","offset":1,"limit":1}"#.into(),
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
}

#[tokio::test]
async fn tool_results_are_replayed_to_the_next_provider_round_before_final_text() {
    let db_path = test_path("roundtrip");
    let _ = fs::remove_file(&db_path);
    let workspace_path = std::env::temp_dir().join(format!(
        "sagent-runtime-tool-actor-workspace-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&workspace_path);
    fs::create_dir_all(&workspace_path).expect("应能创建 workspace");
    fs::write(workspace_path.join("notes.txt"), "第一行\n第二行").expect("应能创建测试文件");

    let session_id = SessionId::new("tool-actor-session");
    create_session(&db_path, &session_id);
    let factory_path = db_path.clone();
    let provider = Arc::new(TwoRoundProvider {
        calls: AtomicUsize::new(0),
    });
    let worker = ToolWorker::new(
        WorkspaceRoot::new(&workspace_path).expect("workspace 应有效"),
        ReadFileLimits::default(),
        TerminalLimits::default(),
    );
    let dispatcher = ToolDispatcher::new(tool_registry());
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider, "mock", "profile-v1")
    .with_tool_dispatcher(dispatcher)
    .with_tool_worker(worker);
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    let mut events = handle.subscribe();
    handle
        .submit(
            RequestId::new(),
            UserInput::new("读取 notes.txt").expect("输入有效"),
        )
        .await
        .expect("提交应成功");

    let mut saw_started = false;
    let mut saw_completed = false;
    let mut saw_completed_turn = false;
    for _ in 0..16 {
        let event = events.recv().await.expect("事件通道应可用");
        match event.kind {
            RuntimeEventKind::ToolStarted { ref call_id, .. } if call_id == "call_read_1" => {
                saw_started = true
            }
            RuntimeEventKind::ToolCompleted {
                ref call_id,
                ok: true,
                ..
            } if call_id == "call_read_1" => saw_completed = true,
            RuntimeEventKind::TurnCompleted => {
                saw_completed_turn = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_started);
    assert!(saw_completed);
    assert!(saw_completed_turn);

    let store = Store::open_readonly(&db_path).expect("应能读取数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].finish_reason.as_deref(), Some("tool_calls"));
    assert!(
        messages[1]
            .tool_calls
            .as_deref()
            .unwrap()
            .contains("call_read_1")
    );
    assert_eq!(messages[2].role, "tool");
    assert_eq!(messages[2].tool_call_id.as_deref(), Some("call_read_1"));
    assert_eq!(messages[2].content, "1|第一行");
    assert_eq!(messages[3].role, "assistant");
    assert_eq!(messages[3].content, "文件第一行是：第一行");

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn tool_loop_stops_before_persisting_a_call_beyond_the_configured_limit() {
    let db_path = test_path("loop-limit");
    let _ = fs::remove_file(&db_path);
    let workspace_path = std::env::temp_dir().join(format!(
        "sagent-runtime-tool-actor-loop-workspace-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&workspace_path);
    fs::create_dir_all(&workspace_path).expect("应能创建 workspace");
    fs::write(workspace_path.join("notes.txt"), "第一行").expect("应能创建测试文件");

    let session_id = SessionId::new("tool-loop-limit-session");
    create_session(&db_path, &session_id);
    let factory_path = db_path.clone();
    let provider = Arc::new(LoopingToolProvider {
        calls: AtomicUsize::new(0),
    });
    let worker = ToolWorker::new(
        WorkspaceRoot::new(&workspace_path).expect("workspace 应有效"),
        ReadFileLimits::default(),
        TerminalLimits::default(),
    );
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider.clone(), "mock", "profile-v1")
    .with_tool_dispatcher(ToolDispatcher::new(tool_registry()))
    .with_tool_worker(worker)
    .with_max_tool_rounds(2);
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    let mut events = handle.subscribe();
    handle
        .submit(
            RequestId::new(),
            UserInput::new("持续读取 notes.txt").expect("输入有效"),
        )
        .await
        .expect("提交应成功");

    let mut failure_reason = None;
    for _ in 0..24 {
        let event = events.recv().await.expect("事件通道应可用");
        if let RuntimeEventKind::TurnFailed { reason } = event.kind {
            failure_reason = Some(reason);
            break;
        }
    }
    assert_eq!(failure_reason.as_deref(), Some("工具调用超过最大回环次数"));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);

    let store = Store::open_readonly(&db_path).expect("应能读取数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(messages.len(), 5);
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.role == "tool")
            .count(),
        2
    );
    assert!(messages.iter().all(|message| {
        message
            .tool_calls
            .as_deref()
            .is_none_or(|calls| !calls.contains("call_loop_3"))
    }));

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn approval_once_resumes_the_paused_terminal_call_and_replays_its_result() {
    let db_path = test_path("approval-once");
    let _ = fs::remove_file(&db_path);
    let workspace_path = std::env::temp_dir().join(format!(
        "sagent-runtime-tool-actor-approval-workspace-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&workspace_path);
    fs::create_dir_all(&workspace_path).expect("应能创建 workspace");

    let session_id = SessionId::new("tool-approval-once-session");
    create_session(&db_path, &session_id);
    let factory_path = db_path.clone();
    let provider = Arc::new(ApprovalProvider {
        calls: AtomicUsize::new(0),
    });
    let worker = ToolWorker::new(
        WorkspaceRoot::new(&workspace_path).expect("workspace 应有效"),
        ReadFileLimits::default(),
        TerminalLimits::default(),
    );
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider.clone(), "mock", "profile-v1")
    .with_tool_dispatcher(ToolDispatcher::new(tool_registry()))
    .with_tool_worker(worker);
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    let mut events = handle.subscribe();
    handle
        .submit(
            RequestId::new(),
            UserInput::new("删除一个不存在的目录").expect("输入有效"),
        )
        .await
        .expect("提交应成功");

    let approval_id = loop {
        let event = events.recv().await.expect("事件通道应可用");
        match event.kind {
            RuntimeEventKind::ApprovalRequested { approval_id, .. } => break approval_id,
            RuntimeEventKind::ToolStarted { .. } => {
                panic!("批准前不得启动危险 terminal")
            }
            _ => {}
        }
    };
    handle
        .resolve_approval(approval_id, ApprovalDecision::Once)
        .await
        .expect("Once 应被接受");

    let mut saw_started = false;
    let mut saw_tool_result = false;
    let mut saw_completed = false;
    for _ in 0..20 {
        let event = events.recv().await.expect("事件通道应可用");
        match event.kind {
            RuntimeEventKind::ToolStarted { ref call_id, .. }
                if call_id == "call_terminal_approval" =>
            {
                saw_started = true;
            }
            RuntimeEventKind::ToolCompleted { ref call_id, .. }
                if call_id == "call_terminal_approval" =>
            {
                saw_tool_result = true;
            }
            RuntimeEventKind::TurnCompleted => {
                saw_completed = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_started);
    assert!(saw_tool_result);
    assert!(saw_completed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);

    let store = Store::open_readonly(&db_path).expect("应能读取数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[2].role, "tool");
    assert_eq!(
        messages[2].tool_call_id.as_deref(),
        Some("call_terminal_approval")
    );
    assert_eq!(messages[3].content, "审批后的工具结果已收到");

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn approval_denial_persists_a_tool_error_without_starting_terminal() {
    let db_path = test_path("approval-deny");
    let _ = fs::remove_file(&db_path);
    let workspace_path = std::env::temp_dir().join(format!(
        "sagent-runtime-tool-actor-approval-deny-workspace-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&workspace_path);
    fs::create_dir_all(&workspace_path).expect("应能创建 workspace");

    let session_id = SessionId::new("tool-approval-deny-session");
    create_session(&db_path, &session_id);
    let factory_path = db_path.clone();
    let provider = Arc::new(ApprovalProvider {
        calls: AtomicUsize::new(0),
    });
    let worker = ToolWorker::new(
        WorkspaceRoot::new(&workspace_path).expect("workspace 应有效"),
        ReadFileLimits::default(),
        TerminalLimits::default(),
    );
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(provider.clone(), "mock", "profile-v1")
    .with_tool_dispatcher(ToolDispatcher::new(tool_registry()))
    .with_tool_worker(worker);
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    let mut events = handle.subscribe();
    handle
        .submit(
            RequestId::new(),
            UserInput::new("删除一个不存在的目录").expect("输入有效"),
        )
        .await
        .expect("提交应成功");

    let approval_id = loop {
        let event = events.recv().await.expect("事件通道应可用");
        if let RuntimeEventKind::ApprovalRequested { approval_id, .. } = event.kind {
            break approval_id;
        }
        assert!(
            !matches!(event.kind, RuntimeEventKind::ToolStarted { .. }),
            "拒绝前不得启动 terminal"
        );
    };
    handle
        .resolve_approval(approval_id, ApprovalDecision::Deny)
        .await
        .expect("Deny 应被接受");

    let mut saw_started = false;
    let mut saw_denied_result = false;
    let mut failure_reason = None;
    for _ in 0..12 {
        let event = events.recv().await.expect("事件通道应可用");
        match event.kind {
            RuntimeEventKind::ToolStarted { .. } => saw_started = true,
            RuntimeEventKind::ToolCompleted {
                ok: false,
                error_kind: Some(ref kind),
                ..
            } if kind == "approval_denied" => saw_denied_result = true,
            RuntimeEventKind::TurnFailed { reason } => {
                failure_reason = Some(reason);
                break;
            }
            _ => {}
        }
    }
    assert!(!saw_started);
    assert!(saw_denied_result);
    assert_eq!(failure_reason.as_deref(), Some("用户拒绝了工具执行"));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

    let store = Store::open_readonly(&db_path).expect("应能读取数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[2].role, "tool");
    assert_eq!(messages[2].content, "用户拒绝了工具执行");
    assert_eq!(
        messages[2].tool_call_id.as_deref(),
        Some("call_terminal_approval")
    );

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn interrupt_cancels_running_tool_without_persisting_a_late_result() {
    let db_path = test_path("tool-interrupt");
    let _ = fs::remove_file(&db_path);
    let workspace_path = std::env::temp_dir().join(format!(
        "sagent-runtime-tool-actor-interrupt-workspace-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&workspace_path);
    fs::create_dir_all(&workspace_path).expect("应能创建 workspace");

    let session_id = SessionId::new("tool-interrupt-session");
    create_session(&db_path, &session_id);
    let factory_path = db_path.clone();
    let worker = ToolWorker::new(
        WorkspaceRoot::new(&workspace_path).expect("workspace 应有效"),
        ReadFileLimits::default(),
        TerminalLimits::default(),
    );
    let worker_probe = worker.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(Arc::new(LongTerminalProvider), "mock", "profile-v1")
    .with_tool_dispatcher(ToolDispatcher::new(tool_registry()))
    .with_tool_worker(worker);
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    let mut events = handle.subscribe();
    handle
        .submit(
            RequestId::new(),
            UserInput::new("运行长命令后取消").expect("输入有效"),
        )
        .await
        .expect("提交应成功");

    loop {
        let event = events.recv().await.expect("事件通道应可用");
        if matches!(
            event.kind,
            RuntimeEventKind::ToolStarted { ref call_id, .. } if call_id == "call_long_terminal"
        ) {
            break;
        }
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if worker_probe.terminal().supervisor().active_count() > 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("terminal 应在取消前启动");

    handle
        .interrupt(RequestId::new())
        .await
        .expect("interrupt 应被接受");
    let interrupted = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.expect("事件通道应可用");
            if matches!(event.kind, RuntimeEventKind::TurnInterrupted) {
                return;
            }
        }
    })
    .await;
    assert!(interrupted.is_ok(), "应在等待进程清理后发布 interrupted");
    assert_eq!(worker_probe.terminal().supervisor().active_count(), 0);

    let store = Store::open_readonly(&db_path).expect("应能读取数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(messages.len(), 2);
    assert!(
        messages
            .iter()
            .all(|message| message.tool_call_id.as_deref() != Some("call_long_terminal"))
    );

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(workspace_path);
}

#[tokio::test]
async fn approval_timeout_wins_without_starting_terminal_or_accepting_a_late_decision() {
    let db_path = test_path("approval-timeout");
    let _ = fs::remove_file(&db_path);
    let workspace_path = std::env::temp_dir().join(format!(
        "sagent-runtime-tool-actor-approval-timeout-workspace-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&workspace_path);
    fs::create_dir_all(&workspace_path).expect("应能创建 workspace");

    let session_id = SessionId::new("tool-approval-timeout-session");
    create_session(&db_path, &session_id);
    let factory_path = db_path.clone();
    let worker = ToolWorker::new(
        WorkspaceRoot::new(&workspace_path).expect("workspace 应有效"),
        ReadFileLimits::default(),
        TerminalLimits::default(),
    );
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    })
    .with_provider(
        Arc::new(ApprovalProvider {
            calls: AtomicUsize::new(0),
        }),
        "mock",
        "profile-v1",
    )
    .with_tool_dispatcher(ToolDispatcher::new(tool_registry()))
    .with_tool_worker(worker)
    .with_approval_timeout(Duration::from_millis(20));
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动 actor");
    let mut events = handle.subscribe();
    handle
        .submit(
            RequestId::new(),
            UserInput::new("等待审批超时").expect("输入有效"),
        )
        .await
        .expect("提交应成功");

    let approval_id = loop {
        let event = events.recv().await.expect("事件通道应可用");
        if let RuntimeEventKind::ApprovalRequested { approval_id, .. } = event.kind {
            break approval_id;
        }
        assert!(!matches!(event.kind, RuntimeEventKind::ToolStarted { .. }));
    };
    let mut saw_tool_started = false;
    let mut saw_timeout = false;
    let mut failure_reason = None;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("事件通道应可用");
            match event.kind {
                RuntimeEventKind::ToolStarted { .. } => saw_tool_started = true,
                RuntimeEventKind::ApprovalTimedOut { .. } => saw_timeout = true,
                RuntimeEventKind::TurnFailed { reason } => {
                    failure_reason = Some(reason);
                    return;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("审批 timeout 应及时收口 Turn");
    assert!(saw_timeout);
    assert!(!saw_tool_started);
    assert_eq!(failure_reason.as_deref(), Some("审批等待超时"));
    assert!(
        handle
            .resolve_approval(approval_id, ApprovalDecision::Once)
            .await
            .is_err()
    );

    let store = Store::open_readonly(&db_path).expect("应能读取数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[2].role, "tool");
    assert!(
        messages[2]
            .display_metadata
            .as_deref()
            .is_some_and(|metadata| metadata.contains("approval_timeout"))
    );

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(workspace_path);
}
