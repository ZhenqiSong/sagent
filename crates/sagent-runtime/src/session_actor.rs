//! 单个 Session 的串行 Actor。
//!
//! SQLite 是持久化事实源；Actor 只在 Repository 成功提交后更新快照并发布事件。
//! 同步 Repository 调用在专用 blocking task 中执行，不阻塞 Tokio worker。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 5 Session Actor

use std::sync::{Arc, Mutex};

use sagent_session::{AppendMessage, DatabaseConnection, MessageRange, Repository};
use sagent_types::ids::SessionId;
use tokio::sync::{mpsc, oneshot};

use crate::error::ActorError;
use crate::event_bus::{EventBus, SessionEvent};
use crate::session_command::{ActorCommand, CommandReply};
use crate::session_snapshot::SessionSnapshot;

/// Session Actor 的可复制句柄。
#[derive(Clone, Debug)]
pub struct SessionHandle {
    session_id: SessionId,
    sender: mpsc::Sender<ActorCommand>,
}

impl SessionHandle {
    /// 返回关联的 Session ID。
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// 获取当前 Actor 快照。
    pub async fn snapshot(&self) -> Result<SessionSnapshot, ActorError> {
        match self.request(|reply| ActorCommand::GetSnapshot { reply }).await? {
            CommandReply::Snapshot(snapshot) => Ok(snapshot),
            _ => unreachable!("GetSnapshot reply type mismatch"),
        }
    }

    /// 追加消息；成功后才会发布 `message.appended` 事件。
    pub async fn append_message(
        &self,
        input: AppendMessage,
    ) -> Result<sagent_types::message::Message, ActorError> {
        match self.request(|reply| ActorCommand::AppendMessage { input, reply }).await? {
            CommandReply::Message(message) => Ok(message),
            _ => unreachable!("AppendMessage reply type mismatch"),
        }
    }

    /// 读取当前消息窗口。
    pub async fn list_messages(
        &self,
        range: MessageRange,
    ) -> Result<Vec<sagent_types::message::Message>, ActorError> {
        match self.request(|reply| ActorCommand::ListMessages { range, reply }).await? {
            CommandReply::Messages(messages) => Ok(messages),
            _ => unreachable!("ListMessages reply type mismatch"),
        }
    }

    /// 关闭 Session。
    pub async fn close(
        &self,
        reason: Option<String>,
    ) -> Result<sagent_types::session::Session, ActorError> {
        match self.request(|reply| ActorCommand::Close { reason, reply }).await? {
            CommandReply::Closed(session) => Ok(session),
            _ => unreachable!("Close reply type mismatch"),
        }
    }

    /// 订阅后续 live event；不补发历史事件。
    pub async fn subscribe(&self) -> Result<crate::event_bus::EventReceiver, ActorError> {
        match self.request(|reply| ActorCommand::Subscribe { reply }).await? {
            CommandReply::Subscription(receiver) => Ok(receiver),
            _ => unreachable!("Subscribe reply type mismatch"),
        }
    }

    /// 主动断开指定订阅；receiver drop 也会自动清理。
    pub async fn detach_subscriber(&self, subscriber_id: u64) -> Result<(), ActorError> {
        match self
            .request(|reply| ActorCommand::DetachSubscriber {
                subscriber_id,
                reply,
            })
            .await?
        {
            CommandReply::Ack => Ok(()),
            _ => unreachable!("DetachSubscriber reply type mismatch"),
        }
    }

    /// 等待 Actor 处理已接受命令后退出。
    pub async fn shutdown(&self) -> Result<(), ActorError> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(ActorCommand::Shutdown { reply })
            .await
            .map_err(|_| ActorError::Shutdown(self.session_id.clone()))?;
        match result.await.map_err(|_| ActorError::ReplyClosed(self.session_id.clone()))?? {
            CommandReply::Ack => Ok(()),
            _ => unreachable!("Shutdown reply type mismatch"),
        }
    }

    async fn request<F>(&self, build: F) -> Result<CommandReply, ActorError>
    where
        F: FnOnce(oneshot::Sender<Result<CommandReply, ActorError>>) -> ActorCommand,
    {
        let (reply, result) = oneshot::channel();
        self.sender.try_send(build(reply)).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => ActorError::MailboxFull(self.session_id.clone()),
            mpsc::error::TrySendError::Closed(_) => ActorError::Shutdown(self.session_id.clone()),
        })?;
        result.await.map_err(|_| ActorError::ReplyClosed(self.session_id.clone()))?
    }
}

/// Session Actor 的启动器。
pub struct SessionActor {
    session_id: SessionId,
    repository: Arc<Mutex<Repository>>,
    snapshot: SessionSnapshot,
    receiver: mpsc::Receiver<ActorCommand>,
    events: EventBus,
}

impl SessionActor {
    /// 启动一个已恢复 Session Actor，返回句柄和 task join handle。
    pub fn spawn(
        database: DatabaseConnection,
        snapshot: SessionSnapshot,
        mailbox_capacity: usize,
        event_capacity: usize,
    ) -> (SessionHandle, tokio::task::JoinHandle<()>) {
        assert!(mailbox_capacity > 0, "mailbox capacity must be positive");
        let session_id = snapshot.session.id.clone();
        let (sender, receiver) = mpsc::channel(mailbox_capacity);
        let actor = Self {
            session_id: session_id.clone(),
            repository: Arc::new(Mutex::new(Repository::new(database))),
            snapshot,
            receiver,
            events: EventBus::new(event_capacity),
        };
        let handle = SessionHandle { session_id, sender };
        let task = tokio::spawn(actor.run());
        (handle, task)
    }

    async fn run(mut self) {
        while let Some(command) = self.receiver.recv().await {
            let shutdown = matches!(command, ActorCommand::Shutdown { .. });
            self.handle(command).await;
            if shutdown {
                while let Ok(command) = self.receiver.try_recv() {
                    self.reject_after_shutdown(command);
                }
                break;
            }
        }
    }

    fn reject_after_shutdown(&self, command: ActorCommand) {
        let error = || ActorError::Shutdown(self.session_id.clone());
        match command {
            ActorCommand::GetSnapshot { reply }
            | ActorCommand::Subscribe { reply }
            | ActorCommand::DetachSubscriber { reply, .. }
            | ActorCommand::Shutdown { reply }
            | ActorCommand::AppendMessage { reply, .. }
            | ActorCommand::ListMessages { reply, .. }
            | ActorCommand::Close { reply, .. } => {
                let _ = reply.send(Err(error()));
            },
        }
    }

    async fn handle(&mut self, command: ActorCommand) {
        match command {
            ActorCommand::GetSnapshot { reply } => {
                let _ = reply.send(Ok(CommandReply::Snapshot(self.snapshot.clone())));
            },
            ActorCommand::AppendMessage { input, reply } => {
                let result = self.append_message(input).await;
                if let Ok(message) = &result {
                    let seq = self.events.next_sequence();
                    self.events.publish(SessionEvent::MessageAppended {
                        message: message.clone(),
                        revision: self.snapshot.session.revision,
                        seq,
                    });
                }
                let _ = reply.send(result.map(CommandReply::Message));
            },
            ActorCommand::ListMessages { range, reply } => {
                let result = self.list_messages(range).await.map(CommandReply::Messages);
                let _ = reply.send(result);
            },
            ActorCommand::Close { reason, reply } => {
                let result = self.close(reason).await.map(CommandReply::Closed);
                let _ = reply.send(result);
            },
            ActorCommand::Subscribe { reply } => {
                let receiver = self.events.subscribe();
                let _ = reply.send(Ok(CommandReply::Subscription(receiver)));
            },
            ActorCommand::DetachSubscriber {
                subscriber_id,
                reply,
            } => {
                self.events.detach(subscriber_id);
                let _ = reply.send(Ok(CommandReply::Ack));
            },
            ActorCommand::Shutdown { reply } => {
                let _ = reply.send(Ok(CommandReply::Ack));
            },
        }
    }

    async fn append_message(
        &mut self,
        input: AppendMessage,
    ) -> Result<sagent_types::message::Message, ActorError> {
        let repository = Arc::clone(&self.repository);
        let session_id = self.session_id.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut repository = repository.lock().map_err(|_| {
                sagent_session::RepositoryError::InvalidInput("Repository 锁已损坏".to_string())
            })?;
            let message = repository.append_message(&session_id, input)?;
            let session = repository
                .get_session(&session_id)?
                .ok_or_else(|| sagent_session::RepositoryError::NotFound(session_id.clone()))?;
            Ok::<_, sagent_session::RepositoryError>((message, session))
        })
        .await
        .map_err(|error| {
            ActorError::Repository(sagent_session::RepositoryError::InvalidInput(
                error.to_string(),
            ))
        })??;
        let (message, session) = result;
        self.snapshot.session = session;
        self.snapshot.messages.push(message.clone());
        Ok(message)
    }

    async fn list_messages(
        &mut self,
        range: MessageRange,
    ) -> Result<Vec<sagent_types::message::Message>, ActorError> {
        let repository = Arc::clone(&self.repository);
        let session_id = self.session_id.clone();
        tokio::task::spawn_blocking(move || {
            let repository = repository.lock().map_err(|_| {
                sagent_session::RepositoryError::InvalidInput("Repository 锁已损坏".to_string())
            })?;
            repository.get_messages(&session_id, range)
        })
        .await
        .map_err(|error| {
            ActorError::Repository(sagent_session::RepositoryError::InvalidInput(
                error.to_string(),
            ))
        })?
        .map_err(ActorError::from)
    }

    async fn close(
        &mut self,
        reason: Option<String>,
    ) -> Result<sagent_types::session::Session, ActorError> {
        let repository = Arc::clone(&self.repository);
        let session_id = self.session_id.clone();
        let session = tokio::task::spawn_blocking(move || {
            let mut repository = repository.lock().map_err(|_| {
                sagent_session::RepositoryError::InvalidInput("Repository 锁已损坏".to_string())
            })?;
            repository.close_session(&session_id, reason.as_deref())
        })
        .await
        .map_err(|error| {
            ActorError::Repository(sagent_session::RepositoryError::InvalidInput(
                error.to_string(),
            ))
        })??;
        self.snapshot.session = session.clone();
        let seq = self.events.next_sequence();
        self.events.publish(SessionEvent::Closed {
            session: session.clone(),
            seq,
        });
        Ok(session)
    }
}
