//! Runtime Supervisor 和 Session Actor registry。
//!
//! Supervisor 是跨 Session 生命周期的唯一所有者：它管理 Repository、live Actor handle、
//! Actor task 和 shutdown 状态，但不复制 Session history 到全局 registry。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 6 Supervisor

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sagent_config::{Config, ConfigPaths, DatabaseConfig};
use sagent_session::{
    CreateSession, DatabaseConnection, ListSessions, Repository, RepositoryError, SessionSummary,
};
use sagent_types::ids::SessionId;
use tokio::sync::Mutex as AsyncMutex;

use crate::error::RuntimeError;
use crate::recovery::recover_session;
use crate::session_actor::{SessionActor, SessionHandle};
use crate::session_snapshot::SessionSnapshot;

struct LiveActor {
    handle: SessionHandle,
    task: tokio::task::JoinHandle<()>,
}

struct RuntimeState {
    accepting: bool,
    actors: HashMap<SessionId, LiveActor>,
}

struct RuntimeInner {
    repository: Arc<Mutex<Option<Repository>>>,
    database_path: PathBuf,
    database_config: DatabaseConfig,
    mailbox_capacity: usize,
    event_capacity: usize,
    max_live_sessions: usize,
    shutdown_timeout: Duration,
    state: Mutex<RuntimeState>,
    lifecycle: AsyncMutex<()>,
}

/// 一个运行中的 Runtime 句柄。
#[derive(Clone)]
pub struct RuntimeHandle {
    inner: Arc<RuntimeInner>,
}

/// `get_session` 返回的 Session 视图。
#[derive(Debug, Clone)]
pub enum SessionView {
    /// Session 已由当前 Runtime 托管。
    Live(SessionHandle),
    /// Session 已持久化但尚未加载为 Actor。
    Snapshot(SessionSnapshot),
}

/// Supervisor 的内部装配器。
pub(crate) struct Supervisor;

impl Supervisor {
    pub(crate) fn open(config: Config, paths: ConfigPaths) -> Result<RuntimeHandle, RuntimeError> {
        config.validate().map_err(|error| RuntimeError::Config(error.to_string()))?;
        let database_path = config
            .database
            .path
            .clone()
            .map(|path| paths.resolve_database_path(path))
            .unwrap_or_else(|| paths.root().join("state.db"));
        let mut database_config = config.database.clone();
        database_config.path = Some(database_path.clone());
        let database = DatabaseConnection::open(&database_path, &database_config)?;
        let repository = Repository::new(database);
        Ok(RuntimeHandle {
            inner: Arc::new(RuntimeInner {
                repository: Arc::new(Mutex::new(Some(repository))),
                database_path,
                database_config,
                mailbox_capacity: config.runtime.actor_mailbox_capacity as usize,
                event_capacity: config.runtime.event_buffer_capacity as usize,
                max_live_sessions: config.runtime.max_live_sessions as usize,
                shutdown_timeout: Duration::from_millis(config.runtime.shutdown_timeout_ms),
                state: Mutex::new(RuntimeState {
                    accepting: true,
                    actors: HashMap::new(),
                }),
                lifecycle: AsyncMutex::new(()),
            }),
        })
    }
}

impl RuntimeHandle {
    /// 创建 Session，并立即装配其 live Actor。
    pub async fn create_session(
        &self,
        input: CreateSession,
    ) -> Result<SessionHandle, RuntimeError> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        self.ensure_accepting()?;
        self.ensure_capacity()?;
        let session =
            self.with_repository(move |repository| repository.create_session(input)).await?;
        self.spawn_actor(session.id).await
    }

    /// 获取 Session；live Session 返回同一个 handle，否则返回已提交快照。
    pub async fn get_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionView>, RuntimeError> {
        self.ensure_accepting()?;
        if let Some(handle) = self.live_handle(session_id)? {
            return Ok(Some(SessionView::Live(handle)));
        }
        let session_id = session_id.clone();
        let snapshot = self
            .with_repository(move |repository| {
                repository
                    .get_session(&session_id)?
                    .map(|_| recover_session(repository, &session_id))
                    .transpose()
            })
            .await?;
        Ok(snapshot.map(SessionView::Snapshot))
    }

    /// 列出已持久化 Session，不加载完整 history。
    pub async fn list_sessions(
        &self,
        query: ListSessions,
    ) -> Result<Vec<SessionSummary>, RuntimeError> {
        self.ensure_accepting()?;
        self.with_repository(move |repository| repository.list_sessions(query)).await
    }

    /// 恢复或取得一个 live Session Actor；重复 resume 不创建第二个 Actor。
    pub async fn resume_session(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionHandle, RuntimeError> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        self.ensure_accepting()?;
        if let Some(handle) = self.live_handle(session_id)? {
            return Ok(handle);
        }
        self.ensure_capacity()?;
        let session_id = session_id.clone();
        let exists = self
            .with_repository({
                let session_id = session_id.clone();
                move |repository| repository.get_session(&session_id)
            })
            .await?;
        if exists.is_none() {
            return Err(RuntimeError::SessionNotFound(session_id));
        }
        self.spawn_actor(session_id).await
    }

    /// 关闭 Runtime；重复调用幂等，超时后不再接受新请求。
    pub async fn shutdown(&self) -> Result<(), RuntimeError> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        {
            let mut state = self.lock_state()?;
            if !state.accepting && state.actors.is_empty() {
                self.take_repository()?;
                return Ok(());
            }
            state.accepting = false;
        }
        let actors = {
            let mut state = self.lock_state()?;
            state.actors.drain().map(|(_, actor)| actor).collect::<Vec<_>>()
        };
        let deadline = tokio::time::Instant::now() + self.inner.shutdown_timeout;
        for actor in actors {
            let mut task = actor.task;
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                task.abort();
                let _ = task.await;
                return Err(RuntimeError::ShutdownTimeout);
            }
            tokio::time::timeout(remaining, actor.handle.shutdown())
                .await
                .map_err(|_| {
                    task.abort();
                    RuntimeError::ShutdownTimeout
                })?
                .map_err(|error| {
                    task.abort();
                    RuntimeError::ActorJoin(error.to_string())
                })?;
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                task.abort();
                let _ = task.await;
                return Err(RuntimeError::ShutdownTimeout);
            }
            match tokio::time::timeout(remaining, &mut task).await {
                Ok(result) => result.map_err(|error| RuntimeError::ActorJoin(error.to_string()))?,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    return Err(RuntimeError::ShutdownTimeout);
                },
            }
        }
        self.take_repository()?;
        Ok(())
    }

    /// 返回 Runtime 使用的数据库路径。
    pub fn database_path(&self) -> &Path {
        &self.inner.database_path
    }

    fn ensure_accepting(&self) -> Result<(), RuntimeError> {
        if !self.lock_state()?.accepting {
            return Err(RuntimeError::ShuttingDown);
        }
        Ok(())
    }

    fn ensure_capacity(&self) -> Result<(), RuntimeError> {
        if self.lock_state()?.actors.len() >= self.inner.max_live_sessions {
            return Err(RuntimeError::MaxLiveSessions);
        }
        Ok(())
    }

    fn live_handle(&self, session_id: &SessionId) -> Result<Option<SessionHandle>, RuntimeError> {
        Ok(self.lock_state()?.actors.get(session_id).map(|actor| actor.handle.clone()))
    }

    async fn spawn_actor(&self, session_id: SessionId) -> Result<SessionHandle, RuntimeError> {
        let snapshot = {
            let session_id = session_id.clone();
            self.with_repository(move |repository| recover_session(repository, &session_id))
                .await?
        };
        let path = self.inner.database_path.clone();
        let database_config = self.inner.database_config.clone();
        let database =
            tokio::task::spawn_blocking(move || DatabaseConnection::open(path, &database_config))
                .await
                .map_err(|error| RuntimeError::DatabaseOpen(error.to_string()))??;
        let (handle, task) = SessionActor::spawn(
            database,
            snapshot,
            self.inner.mailbox_capacity,
            self.inner.event_capacity,
        );
        self.lock_state()?.actors.insert(
            session_id,
            LiveActor {
                handle: handle.clone(),
                task,
            },
        );
        Ok(handle)
    }

    async fn with_repository<T, F>(&self, operation: F) -> Result<T, RuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Repository) -> Result<T, RepositoryError> + Send + 'static,
    {
        let repository = Arc::clone(&self.inner.repository);
        tokio::task::spawn_blocking(move || {
            let mut repository = repository.lock().map_err(|_| RuntimeError::RepositoryLock)?;
            let repository = repository.as_mut().ok_or(RuntimeError::ShuttingDown)?;
            operation(repository).map_err(RuntimeError::from)
        })
        .await
        .map_err(|error| RuntimeError::RepositoryTask(error.to_string()))?
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, RuntimeState>, RuntimeError> {
        self.inner.state.lock().map_err(|_| RuntimeError::StateLock)
    }

    fn take_repository(&self) -> Result<(), RuntimeError> {
        let mut repository =
            self.inner.repository.lock().map_err(|_| RuntimeError::RepositoryLock)?;
        repository.take();
        Ok(())
    }
}
