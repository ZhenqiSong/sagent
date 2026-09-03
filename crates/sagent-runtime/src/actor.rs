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
                let result = self.handle_command(command);
                let _ = reply_to.send(result);
                is_close
            }
            ActorInput::Worker(event) => {
                self.handle_worker_event(event);
                false
            }
            ActorInput::WorkerExited { turn_id, result } => {
                let _ = (turn_id, result);
                false
            }
        }
    }

    fn handle_command(&mut self, command: SessionCommand) -> Result<CommandReply, RuntimeError> {
        match command {
            SessionCommand::SubmitPrompt { request_id, input } => {
                self.submit_prompt(request_id, input)
            }
            SessionCommand::Close => Ok(CommandReply::Closed),
            SessionCommand::Interrupt { .. } => Err(RuntimeError::InvalidLifecycle(
                "步骤 3 尚未实现 interrupt".into(),
            )),
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
        self.active = Some(ActiveTurn {
            turn_id,
            request_id,
            generation: self.generation,
            state: TurnState::Prompting,
            cancellation,
            worker,
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

    fn handle_worker_event(&mut self, event: WorkerEvent) {
        let (turn_id, kind) = match event {
            WorkerEvent::TextDelta { turn_id, text } => {
                (turn_id, RuntimeEventKind::ModelTextDelta { text })
            }
            WorkerEvent::FinalText { turn_id, .. } => {
                let _ = self.is_active_turn(turn_id);
                return;
            }
            WorkerEvent::Failed { turn_id, reason } => {
                (turn_id, RuntimeEventKind::TurnFailed { reason })
            }
            WorkerEvent::Cancelled { turn_id } => (turn_id, RuntimeEventKind::TurnInterrupted),
        };
        if self.is_active_turn(turn_id) {
            self.publish(RuntimeEvent {
                session_id: self.session_id.clone(),
                turn_id: Some(turn_id),
                request_id: self.active.as_ref().map(|turn| turn.request_id),
                kind,
            });
        }
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

fn utc_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    format!("{seconds:020}")
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use sagent_store::{MessageQuery, NewSession, Store};
    use tokio::sync::{broadcast, mpsc, oneshot};

    use super::SessionActor;
    use crate::{
        event::RuntimeEventKind,
        input::{ActorInput, CommandReply},
    };
    use sagent_agent::{RequestId, SessionCommand, UserInput};
    use sagent_types::SessionId;

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
}
