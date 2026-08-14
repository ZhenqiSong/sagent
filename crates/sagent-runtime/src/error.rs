//! Runtime Supervisor 错误类型。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 6 Runtime 错误

use sagent_session::{DatabaseError, RepositoryError};
use sagent_types::ids::SessionId;

/// Session Actor 对调用者暴露的稳定错误。
#[derive(Debug, thiserror::Error)]
pub enum ActorError {
    /// Repository 操作失败。
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    /// Actor mailbox 已满。
    #[error("Session Actor mailbox 已满: {0:?}")]
    MailboxFull(SessionId),
    /// Actor 已停止或正在停止。
    #[error("Session Actor 已停止: {0:?}")]
    Shutdown(SessionId),
    /// 调用者等待响应的通道已关闭。
    #[error("Session Actor 响应通道已关闭: {0:?}")]
    ReplyClosed(SessionId),
}

/// Runtime 和 Supervisor 对上层暴露的稳定错误。
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// 配置或路径装配失败。
    #[error("Runtime 配置无效: {0}")]
    Config(String),
    /// SQLite 初始化失败。
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// 异步打开 Actor 数据库失败。
    #[error("打开 Actor 数据库失败: {0}")]
    DatabaseOpen(String),
    /// Repository 操作失败。
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    /// Runtime 已进入 shutdown，不再接受新请求。
    #[error("Runtime 正在关闭")]
    ShuttingDown,
    /// live Session 达到配置上限。
    #[error("live Session 数量达到上限")]
    MaxLiveSessions,
    /// Session 不存在。
    #[error("Session 不存在: {0:?}")]
    SessionNotFound(SessionId),
    /// Actor task join 失败。
    #[error("Session Actor task 退出异常: {0}")]
    ActorJoin(String),
    /// Supervisor state 锁损坏。
    #[error("Runtime state 锁已损坏")]
    StateLock,
    /// Supervisor Repository 锁损坏。
    #[error("Runtime Repository 锁已损坏")]
    RepositoryLock,
    /// Repository blocking task 退出异常。
    #[error("Runtime Repository task 退出异常: {0}")]
    RepositoryTask(String),
    /// shutdown 超过配置时限。
    #[error("Runtime shutdown 超时")]
    ShutdownTimeout,
}
