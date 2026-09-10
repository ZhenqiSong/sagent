//! SessionActor 的状态机单元测试。
//!
//! 测试作为 actor 私有子模块，故能验证持久化与事件的顺序契约，同时不扩大生产 API。

use std::{fs, path::PathBuf, sync::Arc};

use sagent_store::{EventQuery, MessageQuery, NewSession, Store};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::actor::SessionActor;
use crate::{
    actor::WorkerFactory,
    approval::ApprovalRequest,
    event::RuntimeEventKind,
    input::{ActorInput, CommandReply, WorkerEvent},
};
use sagent_agent::{RequestId, SessionCommand, UserInput};
use sagent_types::{EventSequence, SessionId, ToolCallId, TurnId};

fn test_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sagent-runtime-actor-{name}-{}.db",
        std::process::id()
    ));
    // Windows 可复用已退出测试进程的 PID；每个测试名不同，故只清理自身固定 fixture
    // 能防止上一次 panic 留下的数据库在下一轮把“创建会话”误判为业务冲突。
    let _ = fs::remove_file(&path);
    path
}

fn prepare_store(path: &std::path::Path, session_id: &SessionId) -> Store {
    let mut store = Store::open_readwrite(path).expect("应能打开测试数据库");
    store
        .create_session(&NewSession {
            id: session_id.clone(),
            source: Some("test".into()),
            model: Some("test-model".into()),
            title: None,
            started_at: "2026-09-03T00:00:00Z".into(),
        })
        .expect("应能创建测试会话");
    store
}

fn fixed_clock() -> String {
    "2026-09-03T00:00:00Z".into()
}

#[tokio::test]
async fn submit_persists_before_publishing_acceptance() {
    let path = test_path("submit");
    let session_id = SessionId::new("actor-submit");
    let store = prepare_store(&path, &session_id);
    let (sender, receiver) = mpsc::channel(8);
    let (events, mut event_receiver) = broadcast::channel(8);
    let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
    let actor_task = tokio::spawn(actor.run());
    let request_id = RequestId::new();
    let (reply_to, reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::SubmitPrompt {
                request_id,
                input: UserInput::new("你好").expect("输入有效"),
            },
            reply_to,
        })
        .await
        .expect("命令应能投递");

    let response = reply.await.expect("Actor 应返回结果").expect("提交应成功");
    let turn_id = match response {
        CommandReply::Accepted { turn_id } => turn_id,
        _ => panic!("应返回 Accepted"),
    };
    let accepted = event_receiver.recv().await.expect("应收到 accepted");
    let persisted = event_receiver.recv().await.expect("应收到 persisted");
    assert!(matches!(accepted.kind, RuntimeEventKind::PromptAccepted));
    assert!(matches!(
        persisted.kind,
        RuntimeEventKind::UserMessagePersisted { .. }
    ));
    assert_eq!(accepted.turn_id, Some(turn_id));

    let (close_tx, close_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::Close,
            reply_to: close_tx,
        })
        .await
        .expect("关闭命令应能投递");
    assert!(matches!(
        close_reply
            .await
            .expect("应返回关闭结果")
            .expect("关闭应成功"),
        CommandReply::Closed
    ));
    actor_task.await.expect("Actor 不应 panic");

    let store = Store::open_readonly(&path).expect("应能重新打开数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "你好");
    assert!(
        store
            .get_generation(&session_id, 0)
            .expect("应能读取 generation")
            .is_some()
    );
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn second_submit_is_rejected_without_a_second_message() {
    let path = test_path("busy");
    let session_id = SessionId::new("actor-busy");
    let store = prepare_store(&path, &session_id);
    let (sender, receiver) = mpsc::channel(8);
    let (events, _event_receiver) = broadcast::channel(8);
    let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
    let actor_task = tokio::spawn(actor.run());

    for text in ["第一条", "第二条"] {
        let (reply_to, reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::SubmitPrompt {
                    request_id: RequestId::new(),
                    input: UserInput::new(text).expect("输入有效"),
                },
                reply_to,
            })
            .await
            .expect("命令应能投递");
        let result = reply.await.expect("Actor 应返回结果");
        if text == "第一条" {
            assert!(matches!(result, Ok(CommandReply::Accepted { .. })));
        } else {
            assert!(matches!(result, Err(crate::RuntimeError::Busy { .. })));
        }
    }

    let (close_tx, close_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::Close,
            reply_to: close_tx,
        })
        .await
        .expect("关闭命令应能投递");
    close_reply
        .await
        .expect("应返回关闭结果")
        .expect("关闭应成功");
    actor_task.await.expect("Actor 不应 panic");

    let store = Store::open_readonly(&path).expect("应能重新打开数据库");
    assert_eq!(
        store
            .get_messages_for_display(&session_id, &MessageQuery::default())
            .expect("应能读取消息")
            .len(),
        1
    );
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn final_text_is_persisted_before_completion_events() {
    let path = test_path("final");
    let session_id = SessionId::new("actor-final");
    let store = prepare_store(&path, &session_id);
    let (sender, receiver) = mpsc::channel(8);
    let (events, mut event_receiver) = broadcast::channel(16);
    let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
    let actor_task = tokio::spawn(actor.run());

    let (reply_to, reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::SubmitPrompt {
                request_id: RequestId::new(),
                input: UserInput::new("生成答案").expect("输入有效"),
            },
            reply_to,
        })
        .await
        .expect("提交应能投递");
    let turn_id = match reply.await.expect("应返回结果").expect("提交应成功") {
        CommandReply::Accepted { turn_id } => turn_id,
        _ => panic!("应返回 Accepted"),
    };
    let _ = event_receiver.recv().await;
    let _ = event_receiver.recv().await;

    sender
        .send(ActorInput::Worker(WorkerEvent::FinalText {
            turn_id,
            text: "这是最终答案".into(),
        }))
        .await
        .expect("最终事件应能投递");
    let persisted = event_receiver.recv().await.expect("应收到持久化事件");
    assert!(matches!(
        persisted.kind,
        RuntimeEventKind::FinalMessagePersisted { .. }
    ));

    // 收到完成消息确认时，Store 中的 assistant 消息和持久化事件已经可读。
    let store = Store::open_readonly(&path).expect("应能重新打开数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].content, "这是最终答案");
    let persisted_events = store
        .events_since(&EventQuery {
            session_id: session_id.clone(),
            after_sequence: EventSequence::new(0).expect("序号有效"),
            limit: 200,
        })
        .expect("应能读取持久化事件");
    assert!(
        persisted_events
            .iter()
            .any(|event| event.event_type == "turn.completed")
    );

    let completed = event_receiver.recv().await.expect("应收到完成事件");
    assert!(matches!(completed.kind, RuntimeEventKind::TurnCompleted));

    let (close_tx, close_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::Close,
            reply_to: close_tx,
        })
        .await
        .expect("关闭应能投递");
    assert!(matches!(
        close_reply
            .await
            .expect("应返回关闭结果")
            .expect("关闭应成功"),
        CommandReply::Closed
    ));
    actor_task.await.expect("Actor 不应 panic");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn interrupt_marks_turn_without_creating_assistant_message() {
    let path = test_path("interrupt");
    let session_id = SessionId::new("actor-interrupt");
    let store = prepare_store(&path, &session_id);
    let (sender, receiver) = mpsc::channel(8);
    let (events, mut event_receiver) = broadcast::channel(16);
    let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
    let actor_task = tokio::spawn(actor.run());

    let (submit_tx, submit_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::SubmitPrompt {
                request_id: RequestId::new(),
                input: UserInput::new("中断我").expect("输入有效"),
            },
            reply_to: submit_tx,
        })
        .await
        .expect("提交应能投递");
    let _turn_id = match submit_reply.await.expect("应返回结果").expect("提交应成功") {
        CommandReply::Accepted { turn_id } => turn_id,
        _ => panic!("应返回 Accepted"),
    };
    let _ = event_receiver.recv().await;
    let _ = event_receiver.recv().await;

    let (interrupt_tx, interrupt_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::Interrupt {
                request_id: RequestId::new(),
            },
            reply_to: interrupt_tx,
        })
        .await
        .expect("中断应能投递");
    assert!(matches!(
        interrupt_reply
            .await
            .expect("应返回中断结果")
            .expect("中断应成功"),
        CommandReply::Interrupted
    ));
    let interrupted = event_receiver.recv().await.expect("应收到中断事件");
    assert!(matches!(
        interrupted.kind,
        RuntimeEventKind::TurnInterrupted
    ));

    let store = Store::open_readonly(&path).expect("应能重新打开数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");

    let (close_tx, close_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::Close,
            reply_to: close_tx,
        })
        .await
        .expect("关闭应能投递");
    close_reply
        .await
        .expect("应返回关闭结果")
        .expect("关闭应成功");
    actor_task.await.expect("Actor 不应 panic");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn failed_worker_marks_turn_failed_without_assistant_message() {
    let path = test_path("failed");
    let session_id = SessionId::new("actor-failed");
    let store = prepare_store(&path, &session_id);
    let (sender, receiver) = mpsc::channel(8);
    let (events, mut event_receiver) = broadcast::channel(16);
    let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
    let actor_task = tokio::spawn(actor.run());

    let (submit_tx, submit_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::SubmitPrompt {
                request_id: RequestId::new(),
                input: UserInput::new("触发失败").expect("输入有效"),
            },
            reply_to: submit_tx,
        })
        .await
        .expect("提交应能投递");
    let turn_id = match submit_reply.await.expect("应返回结果").expect("提交应成功") {
        CommandReply::Accepted { turn_id } => turn_id,
        _ => panic!("应返回 Accepted"),
    };
    let _ = event_receiver.recv().await;
    let _ = event_receiver.recv().await;

    sender
        .send(ActorInput::Worker(WorkerEvent::Failed {
            turn_id,
            reason: "provider unavailable".into(),
        }))
        .await
        .expect("失败事件应能投递");
    let failed = event_receiver.recv().await.expect("应收到失败事件");
    assert!(matches!(
        failed.kind,
        RuntimeEventKind::TurnFailed { ref reason } if reason == "provider unavailable"
    ));

    let store = Store::open_readonly(&path).expect("应能重新打开数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(messages.len(), 1);

    let (close_tx, close_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::Close,
            reply_to: close_tx,
        })
        .await
        .expect("关闭应能投递");
    close_reply
        .await
        .expect("应返回关闭结果")
        .expect("关闭应成功");
    actor_task.await.expect("Actor 不应 panic");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn worker_panic_is_converted_to_failed_turn() {
    let path = test_path("panic");
    let session_id = SessionId::new("actor-panic");
    let store = prepare_store(&path, &session_id);
    let (sender, receiver) = mpsc::channel(8);
    let (events, mut event_receiver) = broadcast::channel(16);
    let factory: WorkerFactory = Arc::new(
        |_sender: mpsc::Sender<ActorInput>, _turn_id: TurnId, _token: CancellationToken| {
            tokio::spawn(async { panic!("worker panic") })
        },
    );
    let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events)
        .with_worker_factory(factory, fixed_clock);
    let actor_task = tokio::spawn(actor.run());

    let (submit_tx, submit_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::SubmitPrompt {
                request_id: RequestId::new(),
                input: UserInput::new("触发 panic").expect("输入有效"),
            },
            reply_to: submit_tx,
        })
        .await
        .expect("提交应能投递");
    submit_reply.await.expect("应返回结果").expect("提交应成功");
    let _ = event_receiver.recv().await;
    let _ = event_receiver.recv().await;
    let failed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let event = event_receiver.recv().await.expect("事件通道不应关闭");
            if matches!(event.kind, RuntimeEventKind::TurnFailed { .. }) {
                break event;
            }
        }
    })
    .await
    .expect("panic 应在超时前转换为失败");
    assert!(matches!(failed.kind, RuntimeEventKind::TurnFailed { .. }));

    let store = Store::open_readonly(&path).expect("应能重新打开数据库");
    assert_eq!(
        store
            .get_messages_for_display(&session_id, &MessageQuery::default())
            .expect("应能读取消息")
            .len(),
        1
    );
    let (close_tx, close_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::Close,
            reply_to: close_tx,
        })
        .await
        .expect("关闭应能投递");
    close_reply
        .await
        .expect("应返回关闭结果")
        .expect("关闭应成功");
    actor_task.await.expect("Actor 不应 panic");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn model_delta_is_realtime_only_and_not_persisted() {
    let path = test_path("delta-only");
    let session_id = SessionId::new("actor-delta-only");
    let store = prepare_store(&path, &session_id);
    let (sender, receiver) = mpsc::channel(8);
    let (events, mut event_receiver) = broadcast::channel(16);
    let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
    let actor_task = tokio::spawn(actor.run());

    let (submit_tx, submit_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::SubmitPrompt {
                request_id: RequestId::new(),
                input: UserInput::new("流式输出").expect("输入有效"),
            },
            reply_to: submit_tx,
        })
        .await
        .expect("提交应能投递");
    let turn_id = match submit_reply.await.expect("应返回结果").expect("提交应成功") {
        CommandReply::Accepted { turn_id } => turn_id,
        _ => panic!("应返回 Accepted"),
    };
    let _ = event_receiver.recv().await;
    let _ = event_receiver.recv().await;
    let before = Store::open_readonly(&path)
        .expect("应能打开只读 Store")
        .latest_event_sequence(&session_id)
        .expect("应能读取事件序号")
        .expect("提交后应有事件");

    sender
        .send(ActorInput::Worker(WorkerEvent::TextDelta {
            turn_id,
            text: "实时片段".into(),
        }))
        .await
        .expect("delta 应能投递");
    let delta = event_receiver.recv().await.expect("应收到 delta");
    assert!(matches!(
        delta.kind,
        RuntimeEventKind::ModelTextDelta { .. }
    ));

    let store = Store::open_readonly(&path).expect("应能重新打开数据库");
    let after = store
        .latest_event_sequence(&session_id)
        .expect("应能读取事件序号")
        .expect("提交后应有事件");
    assert_eq!(before, after, "delta 不应写入 daemon_events");

    let events = store
        .events_since(&EventQuery {
            session_id: session_id.clone(),
            after_sequence: EventSequence::new(0).expect("序号有效"),
            limit: 200,
        })
        .expect("应能读取事件历史");
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "model.text.delta")
    );

    let (close_tx, close_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::Close,
            reply_to: close_tx,
        })
        .await
        .expect("关闭应能投递");
    close_reply
        .await
        .expect("应返回关闭结果")
        .expect("关闭应成功");
    actor_task.await.expect("Actor 不应 panic");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn final_wins_over_a_later_interrupt_and_late_command_has_no_side_effect() {
    let path = test_path("race");
    let session_id = SessionId::new("actor-race");
    let store = prepare_store(&path, &session_id);
    let (sender, receiver) = mpsc::channel(8);
    let (events, mut event_receiver) = broadcast::channel(16);
    let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
    let actor_task = tokio::spawn(actor.run());

    let (submit_tx, submit_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::SubmitPrompt {
                request_id: RequestId::new(),
                input: UserInput::new("竞态测试").expect("输入有效"),
            },
            reply_to: submit_tx,
        })
        .await
        .expect("提交应能投递");
    let turn_id = match submit_reply.await.expect("应返回结果").expect("提交应成功") {
        CommandReply::Accepted { turn_id } => turn_id,
        _ => panic!("应返回 Accepted"),
    };
    let _ = event_receiver.recv().await;
    let _ = event_receiver.recv().await;

    sender
        .send(ActorInput::Worker(WorkerEvent::FinalText {
            turn_id,
            text: "先到的最终结果".into(),
        }))
        .await
        .expect("最终事件应能投递");
    let (interrupt_tx, interrupt_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::Interrupt {
                request_id: RequestId::new(),
            },
            reply_to: interrupt_tx,
        })
        .await
        .expect("中断应能投递");

    let _ = event_receiver.recv().await;
    let completed = event_receiver.recv().await.expect("应收到完成事件");
    assert!(matches!(completed.kind, RuntimeEventKind::TurnCompleted));
    assert!(matches!(
        interrupt_reply.await.expect("应返回中断结果"),
        Err(crate::RuntimeError::NoActiveTurn)
    ));

    let store = Store::open_readonly(&path).expect("应能重新打开数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    assert_eq!(messages.len(), 2, "迟到的 interrupt 不能产生额外消息");
    assert_eq!(messages[1].role, "assistant");

    let (close_tx, close_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::Close,
            reply_to: close_tx,
        })
        .await
        .expect("关闭应能投递");
    close_reply
        .await
        .expect("应返回关闭结果")
        .expect("关闭应成功");
    actor_task.await.expect("Actor 不应 panic");
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn approval_request_keeps_actor_responsive_until_resolve() {
    let path = test_path("approval");
    let session_id = SessionId::new("actor-approval");
    let store = prepare_store(&path, &session_id);
    let (sender, receiver) = mpsc::channel(8);
    let (events, mut event_receiver) = broadcast::channel(16);
    let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
    let actor_task = tokio::spawn(actor.run());

    let (submit_tx, submit_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::SubmitPrompt {
                request_id: RequestId::new(),
                input: UserInput::new("审批测试").expect("输入有效"),
            },
            reply_to: submit_tx,
        })
        .await
        .expect("提交应能投递");
    let turn_id = match submit_reply.await.expect("应返回结果").expect("提交应成功") {
        CommandReply::Accepted { turn_id } => turn_id,
        _ => panic!("应返回 Accepted"),
    };
    let _ = event_receiver.recv().await;
    let _ = event_receiver.recv().await;

    let request = ApprovalRequest {
        approval_id: sagent_types::ApprovalId::new(),
        session_id: session_id.clone(),
        turn_id,
        tool_call_id: ToolCallId::new(),
        tool_name: "terminal".into(),
        summary: "该命令需要审批".into(),
        policy_key: "terminal:recursive_delete".into(),
        expires_at: "2026-09-05T00:00:00Z".into(),
    };
    let approval_id = request.approval_id;
    sender
        .send(ActorInput::ApprovalRequired { turn_id, request })
        .await
        .expect("审批请求应能投递");
    let requested = event_receiver.recv().await.expect("应收到审批事件");
    assert!(matches!(
        requested.kind,
        RuntimeEventKind::ApprovalRequested { approval_id: id, .. } if id == approval_id
    ));

    let (resolve_tx, resolve_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::ResolveApproval {
                approval_id,
                decision: sagent_agent::ApprovalDecision::Once,
            },
            reply_to: resolve_tx,
        })
        .await
        .expect("审批响应应能投递");
    assert!(matches!(
        resolve_reply
            .await
            .expect("应返回审批结果")
            .expect("审批应被接收"),
        CommandReply::ApprovalAccepted
    ));
    let resolved = event_receiver.recv().await.expect("应收到审批完成事件");
    assert!(matches!(
        resolved.kind,
        RuntimeEventKind::ApprovalResolved { approval_id: id, decision: sagent_agent::ApprovalDecision::Once }
            if id == approval_id
    ));
    let persisted = Store::open_readonly(&path)
        .expect("应能读取审批事件")
        .events_since(&EventQuery {
            session_id: session_id.clone(),
            after_sequence: EventSequence::new(0).expect("序号有效"),
            limit: 200,
        })
        .expect("审批事件查询应成功");
    assert!(
        persisted
            .iter()
            .any(|event| event.event_type == "approval.requested")
    );
    assert!(
        persisted
            .iter()
            .any(|event| event.event_type == "approval.resolved")
    );

    let (close_tx, close_reply) = oneshot::channel();
    sender
        .send(ActorInput::Command {
            command: SessionCommand::Close,
            reply_to: close_tx,
        })
        .await
        .expect("关闭应能投递");
    close_reply
        .await
        .expect("应返回关闭结果")
        .expect("关闭应成功");
    actor_task.await.expect("Actor 不应 panic");
    let _ = fs::remove_file(path);
}
