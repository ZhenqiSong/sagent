//! Runtime 的统一错误边界。

use sagent_types::SessionId;
use thiserror::Error;

/// SessionActor、Supervisor 和后续 worker 之间的稳定错误分类。
///
/// 这里不直接暴露 rusqlite::Error 或 provider 的专属错误类型；
/// 上层只依赖运行时语义，具体原因通过字符串保留。
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// 当前会话已经有一个正在执行的 Turn。
    #[error("session {session_id:?} is busy")]
    Busy { session_id: SessionId },

    /// actor mailbox 达到容量上限，调用者需要稍后重试。
    #[error("session mailbox is full")]
    MailboxFull,

    /// actor mailbox 已关闭，不能再投递命令。
    #[error("session mailbox is closed")]
    MailboxClosed,

    /// 请求中断时没有活跃 Turn。
    #[error("session has no active turn")]
    NoActiveTurn,

    /// actor 已经退出，旧句柄不再有效。
    #[error("session actor has stopped")]
    ActorStopped,

    /// Store 原子操作失败。
    #[error("persistence failed: {0}")]
    Persistence(String),

    /// 当前生命周期不允许该操作。
    #[error("invalid session lifecycle: {0}")]
    InvalidLifecycle(String),

    /// PromptSnapshot/generation 的稳定 hash 需要显式 transition。
    #[error("session requires an explicit transition")]
    RequiresTransition,

    /// 受监管的 provider/tool worker 发生异常。
    #[error("worker task failed: {0}")]
    WorkerFailed(String),
}

#[cfg(test)]
mod tests {
    use super::RuntimeError;
    use sagent_types::SessionId;

    #[test]
    fn busy_error_contains_session_context() {
        let session_id = SessionId::new("session-1");
        let error = RuntimeError::Busy {
            session_id: session_id.clone(),
        };

        let message = error.to_string();

        assert!(message.contains("busy"));
        assert!(message.contains("session-1"));
    }

    #[test]
    fn persistence_error_keeps_a_readable_cause() {
        let error = RuntimeError::Persistence("transaction rolled back".into());

        assert_eq!(
            error.to_string(),
            "persistence failed: transaction rolled back"
        );
    }

    #[test]
    fn lifecycle_errors_have_distinct_messages() {
        assert_ne!(
            RuntimeError::NoActiveTurn.to_string(),
            RuntimeError::ActorStopped.to_string()
        );
    }
}
