//! 多会话 Supervisor：为每个 SessionId 启动并托管唯一的 SessionActor。
//!
//! 步骤 4 的职责：
//! - 维护 `SessionId -> ManagedSession` 映射，同一 Session 只有一个 actor；
//! - 用固定容量（32）的有界 mailbox 串行化每个 Session 的命令；
//! - actor 退出后清理 stale 条目，旧 handle 返回 `ActorStopped`；
//! - 不同 Session 的 actor 相互独立，Supervisor 不做全局串行。
//!
//! `SessionHandle` 只投递命令并等待一次应答；它不暴露 Store，也不等待整个回合。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sagent_agent::{RequestId, SessionCommand, UserInput};
use sagent_store::Store;
use sagent_types::{SessionId, TurnId};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::RuntimeError;
use crate::actor::{SessionActor, WorkerFactory, utc_now};
use crate::event::{RuntimeEvent, RuntimeEventSubscription};
use crate::input::{ActorInput, CommandReply};

/// 每个 Session 的 mailbox 容量；满时命令立即返回 `MailboxFull`。
const MAILBOX_CAPACITY: usize = 32;
/// 每个 Session 运行时事件广播容量。
const EVENT_CAPACITY: usize = 64;

/// 为单个新 actor 打开独占 Store 的工厂。
///
/// actor 是 Store 的唯一写入者；Supervisor 不会把同一个 Store 交给两个 actor。
/// 打开失败时以可读原因返回，由调用方转换为 `RuntimeError::Persistence`。
type StoreFactory = Arc<dyn Fn() -> Result<Store, String> + Send + Sync>;

/// Supervisor 为每个正在运行的 Session 保存的托管状态。
///
/// 只保存“投递 + 订阅 + 生命周期”三样东西；actor 的可变状态与 Store 都在
/// actor task 内部，这里不可见。
struct ManagedSession {
    command_tx: mpsc::Sender<ActorInput>,
    events: broadcast::Sender<RuntimeEvent>,
    join: JoinHandle<()>,
}

/// 管理多个 SessionActor 的入口。
pub struct SessionSupervisor {
    sessions: Mutex<HashMap<SessionId, ManagedSession>>,
    store_factory: StoreFactory,
    worker_factory: Option<WorkerFactory>,
}

impl SessionSupervisor {
    /// 用 store 工厂创建 Supervisor。
    ///
    /// store 工厂在每个 Session 首次启动时被调用一次，返回该 actor 独占的
    /// 读写 Store。调用方负责解析 DB 路径（后续由 RPC/config 层注入）。
    pub fn new<F>(store_factory: F) -> Self
    where
        F: Fn() -> Result<Store, String> + Send + Sync + 'static,
    {
        Self {
            sessions: Mutex::new(HashMap::new()),
            store_factory: Arc::new(store_factory),
            worker_factory: None,
        }
    }

    /// 取得会话句柄；会话尚未运行时启动一个 actor。
    ///
    /// 同一 Session 的并发调用只会启动一个 actor：map 锁只覆盖检查/插入，
    /// 创建过程是同步的（打开 Store + `tokio::spawn`），因此锁内没有 await。
    pub async fn get_or_start(&self, session_id: SessionId) -> Result<SessionHandle, RuntimeError> {
        let mut guard = self.lock_sessions();
        reap_stale(&mut guard);
        if let Some(managed) = guard.get(&session_id) {
            return Ok(SessionHandle {
                session_id,
                command_tx: managed.command_tx.clone(),
                events: managed.events.clone(),
            });
        }
        let (managed, handle) = self.start_locked(&session_id)?;
        guard.insert(session_id, managed);
        Ok(handle)
    }

    /// 停止并移除一个会话：发送 `Close`、等待 actor 结束，再删除条目。
    ///
    /// 会话本就不在运行时视为幂等成功。已发出的旧句柄此后返回 `ActorStopped`。
    pub async fn remove(&self, session_id: &SessionId) -> Result<(), RuntimeError> {
        let managed = {
            let mut guard = self.lock_sessions();
            reap_stale(&mut guard);
            guard.remove(session_id)
        };
        let Some(managed) = managed else {
            return Ok(());
        };

        let (reply_to, reply) = oneshot::channel();
        let _ = managed
            .command_tx
            .send(ActorInput::Command {
                command: SessionCommand::Close,
                reply_to,
            })
            .await;
        let _ = reply.await;
        let _ = managed.join.await;
        Ok(())
    }

    /// 测试/后续 Provider 注入 worker 工厂（步骤 4 只在测试中使用）。
    #[cfg(test)]
    fn with_worker_factory(mut self, worker_factory: WorkerFactory) -> Self {
        self.worker_factory = Some(worker_factory);
        self
    }

    fn lock_sessions(&self) -> std::sync::MutexGuard<'_, HashMap<SessionId, ManagedSession>> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 在持有 map 锁的情况下启动一个 actor（同步、无 await）。
    fn start_locked(
        &self,
        session_id: &SessionId,
    ) -> Result<(ManagedSession, SessionHandle), RuntimeError> {
        let store = (self.store_factory)().map_err(RuntimeError::Persistence)?;
        let (command_tx, command_rx) = mpsc::channel(MAILBOX_CAPACITY);
        let (event_tx, _) = broadcast::channel(EVENT_CAPACITY);

        let actor = SessionActor::new(
            session_id.clone(),
            store,
            command_rx,
            command_tx.clone(),
            event_tx.clone(),
        );
        let actor = match &self.worker_factory {
            Some(factory) => actor.with_worker_factory(factory.clone(), utc_now),
            None => actor,
        };
        let join = tokio::spawn(actor.run());

        let handle = SessionHandle {
            session_id: session_id.clone(),
            command_tx: command_tx.clone(),
            events: event_tx.clone(),
        };
        let managed = ManagedSession {
            command_tx,
            events: event_tx,
            join,
        };
        Ok((managed, handle))
    }
}

/// 丢弃已经结束的 actor 条目，避免 stale handle 长期占住 map。
fn reap_stale(sessions: &mut HashMap<SessionId, ManagedSession>) {
    sessions.retain(|_, managed| !managed.join.is_finished());
}

/// submit 成功的一次性回执；只确认 Store 已原子提交，不代表回合完成。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SubmitReceipt {
    /// 被接受并持久化的 Turn。
    pub turn_id: TurnId,
}

/// 一个会话 actor 的受限句柄。
///
/// 句柄只负责把命令投递到有界 mailbox，并等待 accepted/busy/closed 等一次应答；
/// 它不等待完整模型回合，也不暴露 Store 或 actor 内部状态。
#[derive(Clone)]
pub struct SessionHandle {
    session_id: SessionId,
    command_tx: mpsc::Sender<ActorInput>,
    events: broadcast::Sender<RuntimeEvent>,
}

impl SessionHandle {
    /// 返回句柄绑定的会话标识。
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// 提交一条用户消息；busy 时返回 `Busy`，mailbox 满时返回 `MailboxFull`。
    pub async fn submit(
        &self,
        request_id: RequestId,
        input: UserInput,
    ) -> Result<SubmitReceipt, RuntimeError> {
        let reply = self.dispatch(SessionCommand::SubmitPrompt { request_id, input })?;
        match reply.await.map_err(|_| RuntimeError::ActorStopped)?? {
            CommandReply::Accepted { turn_id } => Ok(SubmitReceipt { turn_id }),
            CommandReply::Closed | CommandReply::Interrupted => Err(
                RuntimeError::InvalidLifecycle("actor 对 submit 返回了意外的终态应答".into()),
            ),
        }
    }

    /// 请求中断当前回合；actor 侧的取消语义由步骤 5 实现。
    pub async fn interrupt(&self, request_id: RequestId) -> Result<(), RuntimeError> {
        let reply = self.dispatch(SessionCommand::Interrupt { request_id })?;
        match reply.await.map_err(|_| RuntimeError::ActorStopped)?? {
            CommandReply::Interrupted => Ok(()),
            CommandReply::Accepted { .. } | CommandReply::Closed => Err(
                RuntimeError::InvalidLifecycle("actor 对 interrupt 返回了意外的应答".into()),
            ),
        }
    }

    /// 关闭会话 actor；条目清理由 Supervisor 在下次操作或 `remove` 时完成。
    pub async fn close(&self) -> Result<(), RuntimeError> {
        let reply = self.dispatch(SessionCommand::Close)?;
        match reply.await.map_err(|_| RuntimeError::ActorStopped)?? {
            CommandReply::Closed => Ok(()),
            CommandReply::Accepted { .. } | CommandReply::Interrupted => Err(
                RuntimeError::InvalidLifecycle("actor 对 close 返回了意外的应答".into()),
            ),
        }
    }

    /// 订阅本会话的运行时事件（后续步骤用于 TUI/RPC 推送）。
    pub fn subscribe(&self) -> RuntimeEventSubscription {
        RuntimeEventSubscription::new(self.session_id.clone(), self.events.subscribe())
    }

    /// 投递命令到有界 mailbox；满或已关闭时立即返回，不等待 actor。
    fn dispatch(
        &self,
        command: SessionCommand,
    ) -> Result<oneshot::Receiver<Result<CommandReply, RuntimeError>>, RuntimeError> {
        let (reply_to, reply) = oneshot::channel();
        match self
            .command_tx
            .try_send(ActorInput::Command { command, reply_to })
        {
            Ok(()) => Ok(reply),
            Err(mpsc::error::TrySendError::Full(_)) => Err(RuntimeError::MailboxFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(RuntimeError::ActorStopped),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use sagent_agent::{RequestId, SessionCommand, UserInput};
    use sagent_store::{NewSession, Store};
    use sagent_types::SessionId;
    use tokio::sync::{Notify, broadcast, mpsc, oneshot};

    use super::{SessionHandle, SessionSupervisor, SubmitReceipt};
    use crate::actor::WorkerFactory;
    use crate::event::RuntimeEventSubscription;
    use crate::input::{ActorInput, WorkerEvent};
    use crate::{RuntimeError, RuntimeEvent, RuntimeEventKind};

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sagent-runtime-supervisor-{name}-{}.db",
            std::process::id()
        ))
    }

    fn create_sessions(path: &Path, sessions: &[&SessionId]) {
        let mut store = Store::open_readwrite(path).expect("应能打开测试数据库");
        for session in sessions {
            store
                .create_session(&NewSession {
                    id: (*session).clone(),
                    source: Some("supervisor-test".into()),
                    model: Some("test-model".into()),
                    title: None,
                    started_at: "2026-09-03T00:00:00Z".into(),
                })
                .expect("应能创建测试会话");
        }
    }

    fn counting_factory(
        path: PathBuf,
        opens: Arc<AtomicUsize>,
    ) -> impl Fn() -> Result<Store, String> + Send + Sync + 'static {
        move || {
            opens.fetch_add(1, Ordering::SeqCst);
            Store::open_readwrite(&path).map_err(|error| error.to_string())
        }
    }

    async fn wait_for_event(
        receiver: &mut RuntimeEventSubscription,
        predicate: impl Fn(&RuntimeEventKind) -> bool,
    ) -> RuntimeEvent {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = receiver.recv().await.expect("事件通道不应关闭");
                if predicate(&event.kind) {
                    return event;
                }
            }
        })
        .await
        .expect("等待运行时事件超时")
    }

    #[tokio::test]
    async fn concurrent_get_or_start_starts_a_single_actor() {
        let path = test_path("concurrent");
        let _ = fs::remove_file(&path);
        let session_id = SessionId::new("concurrent-session");
        create_sessions(&path, &[&session_id]);
        let opens = Arc::new(AtomicUsize::new(0));
        let supervisor = Arc::new(SessionSupervisor::new(counting_factory(
            path.clone(),
            opens.clone(),
        )));

        let mut tasks = Vec::new();
        for _ in 0..100 {
            let supervisor = supervisor.clone();
            let session_id = session_id.clone();
            tasks.push(tokio::spawn(async move {
                supervisor.get_or_start(session_id).await
            }));
        }
        let mut handles = Vec::new();
        for task in tasks {
            handles.push(
                task.await
                    .expect("并发 get_or_start 不应 panic")
                    .expect("应能取得会话句柄"),
            );
        }
        assert_eq!(
            opens.load(Ordering::SeqCst),
            1,
            "同一 Session 只能启动一个 actor"
        );

        let first = handles[0]
            .submit(
                RequestId::new(),
                UserInput::new("第一条").expect("输入有效"),
            )
            .await;
        assert!(matches!(first, Ok(SubmitReceipt { .. })));
        let second = handles[50]
            .submit(
                RequestId::new(),
                UserInput::new("第二条").expect("输入有效"),
            )
            .await;
        assert!(
            matches!(second, Err(RuntimeError::Busy { .. })),
            "所有句柄应指向同一个 actor，第二条应 busy"
        );

        supervisor
            .remove(&session_id)
            .await
            .expect("remove 应能停止并清理 actor");
        let stale = handles[99]
            .submit(
                RequestId::new(),
                UserInput::new("第三条").expect("输入有效"),
            )
            .await;
        assert!(
            matches!(stale, Err(RuntimeError::ActorStopped)),
            "旧句柄在 actor 退出后应返回 ActorStopped"
        );
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn get_or_start_reuses_live_actor_until_remove() {
        let path = test_path("reuse");
        let _ = fs::remove_file(&path);
        let session_id = SessionId::new("reuse-session");
        create_sessions(&path, &[&session_id]);
        let opens = Arc::new(AtomicUsize::new(0));
        let supervisor = SessionSupervisor::new(counting_factory(path.clone(), opens.clone()));

        let first = supervisor
            .get_or_start(session_id.clone())
            .await
            .expect("首次应启动 actor");
        let second = supervisor
            .get_or_start(session_id.clone())
            .await
            .expect("应复用同一 actor");
        assert_eq!(opens.load(Ordering::SeqCst), 1, "复用期间不应再次启动");
        assert_eq!(first.session_id(), second.session_id());

        supervisor.remove(&session_id).await.expect("remove 应成功");
        let recreated = supervisor
            .get_or_start(session_id.clone())
            .await
            .expect("移除后应能重新启动 actor");
        assert_eq!(opens.load(Ordering::SeqCst), 2, "重新启动应再次打开 Store");

        let stale = first
            .submit(
                RequestId::new(),
                UserInput::new("过期消息").expect("输入有效"),
            )
            .await;
        assert!(matches!(stale, Err(RuntimeError::ActorStopped)));
        let accepted = recreated
            .submit(
                RequestId::new(),
                UserInput::new("新消息").expect("输入有效"),
            )
            .await;
        assert!(matches!(accepted, Ok(SubmitReceipt { .. })));

        supervisor
            .remove(&session_id)
            .await
            .expect("收尾 remove 应成功");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn handle_close_stops_actor_and_stale_entry_is_reaped() {
        let path = test_path("close-reap");
        let _ = fs::remove_file(&path);
        let session_id = SessionId::new("close-session");
        create_sessions(&path, &[&session_id]);
        let opens = Arc::new(AtomicUsize::new(0));
        let supervisor = SessionSupervisor::new(counting_factory(path.clone(), opens.clone()));

        let handle = supervisor
            .get_or_start(session_id.clone())
            .await
            .expect("应能启动 actor");
        handle
            .submit(
                RequestId::new(),
                UserInput::new("进行中的回合").expect("输入有效"),
            )
            .await
            .expect("首个回合应被接受");
        handle.close().await.expect("close 应返回 Closed");

        let stale = handle
            .submit(
                RequestId::new(),
                UserInput::new("关闭后的消息").expect("输入有效"),
            )
            .await;
        assert!(matches!(stale, Err(RuntimeError::ActorStopped)));

        let reopened = supervisor
            .get_or_start(session_id.clone())
            .await
            .expect("close 后应能重新启动新 actor");
        assert_eq!(opens.load(Ordering::SeqCst), 2, "stale 条目应被 reap");
        let accepted = reopened
            .submit(
                RequestId::new(),
                UserInput::new("新回合").expect("输入有效"),
            )
            .await;
        assert!(matches!(accepted, Ok(SubmitReceipt { .. })));

        supervisor
            .remove(&session_id)
            .await
            .expect("收尾 remove 应成功");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn another_session_submits_while_first_session_is_busy() {
        let path = test_path("independent-sessions");
        let _ = fs::remove_file(&path);
        let session_a = SessionId::new("independent-a");
        let session_b = SessionId::new("independent-b");
        create_sessions(&path, &[&session_a, &session_b]);
        let opens = Arc::new(AtomicUsize::new(0));
        let supervisor = SessionSupervisor::new(counting_factory(path.clone(), opens.clone()));

        let handle_a = supervisor
            .get_or_start(session_a.clone())
            .await
            .expect("应能启动会话 A");
        let handle_b = supervisor
            .get_or_start(session_b.clone())
            .await
            .expect("应能启动会话 B");

        handle_a
            .submit(
                RequestId::new(),
                UserInput::new("A 的长回合").expect("输入有效"),
            )
            .await
            .expect("A 应被接受并保持 busy");
        let accepted_b = handle_b
            .submit(
                RequestId::new(),
                UserInput::new("B 的回合").expect("输入有效"),
            )
            .await;
        assert!(
            matches!(accepted_b, Ok(SubmitReceipt { .. })),
            "B 不应被 A 的 busy 回合阻塞"
        );

        supervisor.remove(&session_a).await.expect("移除 A 应成功");
        supervisor.remove(&session_b).await.expect("移除 B 应成功");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn blocked_worker_does_not_block_another_session() {
        let path = test_path("parallel-workers");
        let _ = fs::remove_file(&path);
        let session_a = SessionId::new("parallel-a");
        let session_b = SessionId::new("parallel-b");
        create_sessions(&path, &[&session_a, &session_b]);

        let opens = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let release_a = Arc::new(Notify::new());
        let factory: WorkerFactory = {
            let calls = calls.clone();
            let release_a = release_a.clone();
            Arc::new(move |sender, turn_id, cancellation| {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    let release_a = release_a.clone();
                    tokio::spawn(async move {
                        tokio::select! {
                            _ = release_a.notified() => {}
                            _ = cancellation.cancelled() => {
                                let _ = sender
                                    .send(ActorInput::Worker(WorkerEvent::Cancelled { turn_id }))
                                    .await;
                                return;
                            }
                        }
                        let _ = sender
                            .send(ActorInput::Worker(WorkerEvent::TextDelta {
                                turn_id,
                                text: "a-chunk".into(),
                            }))
                            .await;
                    })
                } else {
                    tokio::spawn(async move {
                        let _ = sender
                            .send(ActorInput::Worker(WorkerEvent::TextDelta {
                                turn_id,
                                text: "b-chunk".into(),
                            }))
                            .await;
                    })
                }
            })
        };

        let supervisor = SessionSupervisor::new(counting_factory(path.clone(), opens))
            .with_worker_factory(factory);
        let handle_a = supervisor
            .get_or_start(session_a.clone())
            .await
            .expect("应能启动会话 A");
        let handle_b = supervisor
            .get_or_start(session_b.clone())
            .await
            .expect("应能启动会话 B");
        let mut events_a = handle_a.subscribe();
        let mut events_b = handle_b.subscribe();

        handle_a
            .submit(
                RequestId::new(),
                UserInput::new("A 的任务").expect("输入有效"),
            )
            .await
            .expect("A 应被接受");
        let _ = wait_for_event(&mut events_a, |kind| {
            matches!(kind, RuntimeEventKind::PromptAccepted)
        })
        .await;
        let _ = wait_for_event(&mut events_a, |kind| {
            matches!(kind, RuntimeEventKind::UserMessagePersisted { .. })
        })
        .await;

        handle_b
            .submit(
                RequestId::new(),
                UserInput::new("B 的任务").expect("输入有效"),
            )
            .await
            .expect("B 不应被 A 的阻塞 worker 挡住");
        let b_delta = wait_for_event(
            &mut events_b,
            |kind| matches!(kind, RuntimeEventKind::ModelTextDelta { text } if text == "b-chunk"),
        )
        .await;
        assert_eq!(b_delta.session_id, session_b);

        // A 的 worker 仍被闸门挡住：在放行前 A 不应产生任何 delta。
        assert!(events_a.try_recv().expect("事件通道不应关闭").is_none());

        release_a.notify_one();
        let a_delta = wait_for_event(
            &mut events_a,
            |kind| matches!(kind, RuntimeEventKind::ModelTextDelta { text } if text == "a-chunk"),
        )
        .await;
        assert_eq!(a_delta.session_id, session_a);

        supervisor.remove(&session_a).await.expect("移除 A 应成功");
        supervisor.remove(&session_b).await.expect("移除 B 应成功");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn full_mailbox_returns_mailbox_full() {
        let (command_tx, _command_rx) = mpsc::channel::<ActorInput>(1);
        let (event_tx, _event_rx) = broadcast::channel(8);
        let handle = SessionHandle {
            session_id: SessionId::new("full-mailbox"),
            command_tx: command_tx.clone(),
            events: event_tx,
        };

        // 填满容量为 1 的 mailbox，使下一条命令立即触发 MailboxFull。
        let (reply_to, _reply) = oneshot::channel();
        command_tx
            .try_send(ActorInput::Command {
                command: SessionCommand::Close,
                reply_to,
            })
            .expect("首条命令应占住 mailbox");

        let result = handle
            .submit(
                RequestId::new(),
                UserInput::new("被拒绝的内容").expect("输入有效"),
            )
            .await;
        assert!(matches!(result, Err(RuntimeError::MailboxFull)));
    }

    #[tokio::test]
    async fn closed_mailbox_maps_to_actor_stopped() {
        let (command_tx, command_rx) = mpsc::channel::<ActorInput>(4);
        drop(command_rx); // 模拟 actor 已经退出
        let (event_tx, _event_rx) = broadcast::channel(8);
        let handle = SessionHandle {
            session_id: SessionId::new("closed-mailbox"),
            command_tx,
            events: event_tx,
        };

        let result = handle
            .submit(
                RequestId::new(),
                UserInput::new("发给已退出 actor").expect("输入有效"),
            )
            .await;
        assert!(matches!(result, Err(RuntimeError::ActorStopped)));
    }

    #[tokio::test]
    async fn store_open_failure_returns_persistence_without_actor() {
        let supervisor = SessionSupervisor::new(|| -> Result<Store, String> {
            Err("无法打开数据库".into())
        });

        let result = supervisor.get_or_start(SessionId::new("no-store")).await;
        assert!(matches!(result, Err(RuntimeError::Persistence(_))));
    }
}
