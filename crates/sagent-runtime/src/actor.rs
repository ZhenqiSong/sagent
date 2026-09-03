//! SessionActor 的最小 submit 处理循环。

use std::sync::Arc;

use sagent_agent::{
    PromptMessage, PromptRole, PromptSnapshot, RequestId, SessionCommand, SystemPromptParts,
    TurnState, UserInput,
};
use sagent_store::{NewGeneration, NewMessage, StartTurn, Store};
use sagent_types::{SessionId, TurnId};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    RuntimeError,
    active_turn::ActiveTurn,
    event::{RuntimeEvent, RuntimeEventKind},
    input::{ActorInput, CommandReply, WorkerEvent},
};

const DEFAULT_MODEL_ID: &str = "unconfigured";
const DEFAULT_PROFILE_REVISION: &str = "runtime-v1";
const EMPTY_TOOL_SCHEMA_HASH: &str = "sha256:empty-tools";

/// 测试替身或后续 Provider 用来启动 worker 的函数。
pub(crate) type WorkerFactory = Arc<
    dyn Fn(mpsc::Sender<ActorInput>, TurnId, CancellationToken) -> JoinHandle<()> + Send + Sync,
>;

/// 一个 Session 的唯一写入者。
pub(crate) struct SessionActor {
    pub(crate) session_id: SessionId,
    pub(crate) store: Store,
    pub(crate) command_rx: mpsc::Receiver<ActorInput>,
    pub(crate) command_tx: mpsc::Sender<ActorInput>,
    pub(crate) event_tx: broadcast::Sender<RuntimeEvent>,
    pub(crate) active: Option<ActiveTurn>,
    pub(crate) generation: i64,
    pub(crate) worker_factory: Option<WorkerFactory>,
    pub(crate) clock: fn() -> String,
}

impl SessionActor {
    /// 创建不启动 worker 的 Actor；步骤 3 的单测可以直接驱动提交边界。
    pub(crate) fn new(
        session_id: SessionId,
        store: Store,
        command_rx: mpsc::Receiver<ActorInput>,
        command_tx: mpsc::Sender<ActorInput>,
        event_tx: broadcast::Sender<RuntimeEvent>,
    ) -> Self {
        Self {
            session_id,
            store,
            command_rx,
            command_tx,
            event_tx,
            active: None,
            generation: 0,
            worker_factory: None,
            clock: utc_now,
        }
    }

    /// 为测试或后续 Provider 注入受监管的 worker 工厂和时钟。
    pub(crate) fn with_worker_factory(
        mut self,
        worker_factory: WorkerFactory,
        clock: fn() -> String,
    ) -> Self {
        self.worker_factory = Some(worker_factory);
        self.clock = clock;
        self
    }

    /// 按顺序消费 mailbox；Store 只在这个循环所属的 Actor 中被写入。
    pub(crate) async fn run(mut self) {
        while let Some(input) = self.command_rx.recv().await {
            if self.handle_input(input).await {
                break;
            }
        }
    }

    async fn handle_input(&mut self, input: ActorInput) -> bool {
        match input {
            ActorInput::Command { command, reply_to } => {
                let is_close = matches!(command, SessionCommand::Close);
                let result = self.handle_command(command).await;
                let _ = reply_to.send(result);
                is_close
            }
            ActorInput::Worker(event) => {
                self.handle_worker_event(event).await;
                false
            }
            ActorInput::WorkerExited { turn_id, result } => {
                self.handle_worker_exited(turn_id, result).await;
                false
            }
        }
    }

    async fn handle_command(
        &mut self,
        command: SessionCommand,
    ) -> Result<CommandReply, RuntimeError> {
        match command {
            SessionCommand::SubmitPrompt { request_id, input } => {
                self.submit_prompt(request_id, input)
            }
            SessionCommand::Close => {
                // Close 在空闲会话上是幂等的；有 active Turn 时才需要先收口。
                if self.active.is_some() {
                    self.interrupt_active("session closing").await?;
                }
                Ok(CommandReply::Closed)
            }
            SessionCommand::Interrupt { .. } => self.interrupt_active("user interrupt").await,
            SessionCommand::ResolveApproval { .. } => Err(RuntimeError::InvalidLifecycle(
                "步骤 3 尚未实现 approval".into(),
            )),
            SessionCommand::Resume { .. } => Err(RuntimeError::InvalidLifecycle(
                "步骤 3 尚未实现 resume".into(),
            )),
        }
    }

    fn submit_prompt(
        &mut self,
        request_id: RequestId,
        input: UserInput,
    ) -> Result<CommandReply, RuntimeError> {
        if self.active.is_some() {
            return Err(RuntimeError::Busy {
                session_id: self.session_id.clone(),
            });
        }

        let turn_id = TurnId::new();
        let system = SystemPromptParts {
            identity: "你是 Sagent。".into(),
            instructions: "请准确回答用户问题。".into(),
            environment: "运行时：sagent-runtime。".into(),
            ..SystemPromptParts::default()
        };
        let messages = vec![
            PromptMessage::new(PromptRole::System, system.render()),
            PromptMessage::new(PromptRole::User, input.as_str()),
        ];
        let snapshot = PromptSnapshot::new(self.session_id.clone(), turn_id, &system, messages)
            .map_err(|error| RuntimeError::InvalidLifecycle(error.to_string()))?;
        let system_hash = snapshot.system_prompt_hash.clone();

        self.ensure_generation(&system_hash)?;

        let timestamp = (self.clock)();
        let message = NewMessage::new(
            self.session_id.clone(),
            "user",
            input.as_str(),
            timestamp.clone(),
        );
        let persisted_message_id = self
            .store
            .begin_turn(
                &StartTurn {
                    turn_id,
                    session_id: self.session_id.clone(),
                    generation: self.generation,
                    started_at: timestamp,
                },
                &message,
            )
            .map_err(|error| RuntimeError::Persistence(error.to_string()))?;

        let cancellation = CancellationToken::new();
        let worker = self
            .worker_factory
            .as_ref()
            .map(|factory| factory(self.command_tx.clone(), turn_id, cancellation.child_token()));
        let worker_abort = worker.as_ref().map(JoinHandle::abort_handle);
        let worker = worker.map(|worker| {
            let sender = self.command_tx.clone();
            tokio::spawn(async move {
                let result = worker
                    .await
                    .map_err(|error| crate::input::WorkerFailure(error.to_string()));
                // 退出事实不能因为 mailbox 暂时拥塞而丢失；stop_worker 会在
                // Actor 已经决定终态时主动 abort 这个监控任务，避免等待发送造成死锁。
                let _ = sender
                    .send(ActorInput::WorkerExited { turn_id, result })
                    .await;
            })
        });
        self.active = Some(ActiveTurn {
            turn_id,
            request_id,
            generation: self.generation,
            state: TurnState::Prompting,
            cancellation,
            worker,
            worker_abort,
            terminal: false,
        });

        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(request_id),
            kind: RuntimeEventKind::PromptAccepted,
        });
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(request_id),
            kind: RuntimeEventKind::UserMessagePersisted {
                message_id: persisted_message_id,
            },
        });

        Ok(CommandReply::Accepted { turn_id })
    }

    fn ensure_generation(&mut self, system_hash: &str) -> Result<(), RuntimeError> {
        match self
            .store
            .get_generation(&self.session_id, self.generation)
            .map_err(|error| RuntimeError::Persistence(error.to_string()))?
        {
            Some(generation)
                if generation.system_hash == system_hash
                    && generation.tool_schema_hash == EMPTY_TOOL_SCHEMA_HASH => {}
            Some(_) => return Err(RuntimeError::RequiresTransition),
            None => self
                .store
                .create_generation(&NewGeneration {
                    session_id: self.session_id.clone(),
                    generation: self.generation,
                    system_hash: system_hash.to_owned(),
                    tool_schema_hash: EMPTY_TOOL_SCHEMA_HASH.to_owned(),
                    model_id: DEFAULT_MODEL_ID.to_owned(),
                    profile_revision: DEFAULT_PROFILE_REVISION.to_owned(),
                    created_at: (self.clock)(),
                })
                .map_err(|error| RuntimeError::Persistence(error.to_string()))?,
        }
        Ok(())
    }

    async fn handle_worker_event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::TextDelta { turn_id, text } => {
                if self.is_active_turn(turn_id) && !self.is_cancelled(turn_id) {
                    self.publish(RuntimeEvent {
                        session_id: self.session_id.clone(),
                        turn_id: Some(turn_id),
                        request_id: self.active.as_ref().map(|turn| turn.request_id),
                        kind: RuntimeEventKind::ModelTextDelta { text },
                    });
                }
            }
            WorkerEvent::FinalText { turn_id, text } => {
                let _ = self.complete_active(turn_id, text).await;
            }
            WorkerEvent::Failed { turn_id, reason } => {
                let _ = self.fail_active(turn_id, "worker", reason).await;
            }
            WorkerEvent::Cancelled { turn_id } => {
                let _ = self
                    .interrupt_active_for_turn(turn_id, "worker cancelled")
                    .await;
            }
        }
    }

    async fn handle_worker_exited(
        &mut self,
        turn_id: TurnId,
        result: Result<(), crate::input::WorkerFailure>,
    ) {
        if !self.is_active_turn(turn_id) || self.is_cancelled(turn_id) {
            return;
        }
        if let Err(error) = result {
            let _ = self.fail_active(turn_id, "worker", error.0).await;
        } else {
            let _ = self
                .fail_active(turn_id, "worker_exit", "worker 未产生最终结果".into())
                .await;
        }
    }

    async fn interrupt_active(&mut self, reason: &str) -> Result<CommandReply, RuntimeError> {
        let turn_id = self
            .active
            .as_ref()
            .map(|active| active.turn_id)
            .ok_or(RuntimeError::NoActiveTurn)?;
        self.interrupt_active_for_turn(turn_id, reason).await
    }

    async fn interrupt_active_for_turn(
        &mut self,
        turn_id: TurnId,
        reason: &str,
    ) -> Result<CommandReply, RuntimeError> {
        let Some(mut active) = self.take_active(turn_id) else {
            return Err(RuntimeError::NoActiveTurn);
        };
        if active.terminal {
            self.active = Some(active);
            return Err(RuntimeError::NoActiveTurn);
        }
        active.cancellation.cancel();
        Self::stop_worker(&mut active).await;
        let timestamp = (self.clock)();
        if let Err(error) = self.store.interrupt_turn(&turn_id, reason, &timestamp) {
            self.active = Some(active);
            return Err(RuntimeError::Persistence(error.to_string()));
        }
        active.terminal = true;
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(active.request_id),
            kind: RuntimeEventKind::TurnInterrupted,
        });
        Ok(CommandReply::Interrupted)
    }

    async fn complete_active(&mut self, turn_id: TurnId, text: String) -> Result<(), RuntimeError> {
        let Some(mut active) = self.take_active(turn_id) else {
            return Ok(());
        };
        if active.terminal || active.cancellation.is_cancelled() {
            self.active = Some(active);
            return Ok(());
        }
        let timestamp = (self.clock)();
        let message = NewMessage::new(
            self.session_id.clone(),
            "assistant",
            text,
            timestamp.clone(),
        );
        let message_id = match self.store.complete_turn(&turn_id, &message, &timestamp) {
            Ok(message_id) => message_id,
            Err(error) => {
                self.active = Some(active);
                return Err(RuntimeError::Persistence(error.to_string()));
            }
        };
        active.terminal = true;
        Self::stop_worker(&mut active).await;
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(active.request_id),
            kind: RuntimeEventKind::FinalMessagePersisted { message_id },
        });
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(active.request_id),
            kind: RuntimeEventKind::TurnCompleted,
        });
        Ok(())
    }

    async fn fail_active(
        &mut self,
        turn_id: TurnId,
        category: &str,
        reason: String,
    ) -> Result<(), RuntimeError> {
        let Some(mut active) = self.take_active(turn_id) else {
            return Ok(());
        };
        if active.terminal {
            self.active = Some(active);
            return Ok(());
        }
        active.cancellation.cancel();
        Self::stop_worker(&mut active).await;
        let timestamp = (self.clock)();
        if let Err(error) = self
            .store
            .fail_turn(&turn_id, category, &reason, &timestamp)
        {
            self.active = Some(active);
            return Err(RuntimeError::Persistence(error.to_string()));
        }
        active.terminal = true;
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(active.request_id),
            kind: RuntimeEventKind::TurnFailed { reason },
        });
        Ok(())
    }

    fn take_active(&mut self, turn_id: TurnId) -> Option<ActiveTurn> {
        if self.is_active_turn(turn_id) {
            self.active.take()
        } else {
            None
        }
    }

    async fn stop_worker(active: &mut ActiveTurn) {
        if let Some(abort) = active.worker_abort.take() {
            abort.abort();
        }
        if let Some(worker) = active.worker.take() {
            // 监控任务可能正阻塞在有界 mailbox 的 send 上，先 abort 它再等待。
            worker.abort();
            let _ = worker.await;
        }
    }

    fn is_cancelled(&self, turn_id: TurnId) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.turn_id == turn_id && active.cancellation.is_cancelled())
    }

    fn is_active_turn(&self, turn_id: TurnId) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.turn_id == turn_id)
    }

    fn publish(&self, event: RuntimeEvent) {
        let _ = self.event_tx.send(event);
    }
}

pub(crate) fn utc_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    format!("{seconds:020}")
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, sync::Arc};

    use sagent_store::{EventQuery, MessageQuery, NewSession, Store};
    use tokio::sync::{broadcast, mpsc, oneshot};
    use tokio_util::sync::CancellationToken;

    use super::SessionActor;
    use crate::{
        actor::WorkerFactory,
        event::RuntimeEventKind,
        input::{ActorInput, CommandReply, WorkerEvent},
    };
    use sagent_agent::{RequestId, SessionCommand, UserInput};
    use sagent_types::{EventSequence, SessionId, TurnId};

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sagent-runtime-actor-{name}-{}.db",
            std::process::id()
        ))
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
        let _turn_id = match submit_reply.await.expect("应返回结果").expect("提交应成功")
        {
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
}
