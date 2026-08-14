//! Session Actor 的内存快照。
//!
//! 快照只在 Repository 事务成功后替换，避免内存状态领先于 SQLite。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 5 Actor 快照

use sagent_types::message::Message;
use sagent_types::session::Session;

/// Actor 当前持有的 Session 和已加载消息。
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    /// Session 持久化投影。
    pub session: Session,
    /// 已恢复的消息，按 sequence 升序排列。
    pub messages: Vec<Message>,
}

impl From<sagent_session::SessionSnapshot> for SessionSnapshot {
    fn from(snapshot: sagent_session::SessionSnapshot) -> Self {
        Self {
            session: snapshot.session,
            messages: snapshot.messages,
        }
    }
}

impl SessionSnapshot {
    /// 从 Repository 恢复一个完整的受限快照。
    pub fn from_repository(
        repository: &mut sagent_session::Repository,
        session_id: &sagent_types::ids::SessionId,
    ) -> Result<Self, sagent_session::RepositoryError> {
        let snapshot = repository.resume_session(session_id)?;
        Ok(Self {
            session: snapshot.session,
            messages: snapshot.messages,
        })
    }
}
