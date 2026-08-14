//! Session Actor 错误类型。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 5 Actor 错误

use sagent_session::RepositoryError;
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
