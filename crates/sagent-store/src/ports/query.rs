//! 会话、消息、generation 和恢复事件的只读查询端口。

use sagent_types::{EventSequence, MessageId, SessionId, SessionSummary, StoredMessage, TurnId};

use crate::{
    EventQuery, MessageQuery, MessageWindow, SessionListQuery, StoredDaemonEvent, StoredGeneration,
    StoredRunningTurn,
};

use super::StorageResult;

/// 会话、消息和恢复事实的最小只读查询端口。
pub trait SessionQueryStorage: Send + Sync {
    /// 按可见性条件和分页读取会话摘要。
    fn list_sessions(&self, query: &SessionListQuery) -> StorageResult<Vec<SessionSummary>>;

    /// 按完整 ID 读取会话摘要。
    fn get_session(&self, session_id: &SessionId) -> StorageResult<Option<SessionSummary>>;

    /// 读取仅供模型使用的活动消息上下文。
    fn get_messages_for_model(
        &self,
        session_id: &SessionId,
        query: &MessageQuery,
    ) -> StorageResult<Vec<StoredMessage>>;

    /// 读取用户可见的会话消息历史。
    fn get_messages_for_display(
        &self,
        session_id: &SessionId,
        query: &MessageQuery,
    ) -> StorageResult<Vec<StoredMessage>>;

    /// 读取指定消息两侧的上下文窗口。
    fn get_messages_around(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        window: u32,
        include_inactive: bool,
    ) -> StorageResult<Option<MessageWindow>>;

    /// 读取会话中唯一的 running Turn。
    fn get_running_turn(&self, session_id: &SessionId) -> StorageResult<Option<StoredRunningTurn>>;

    /// 读取指定 generation 快照。
    fn get_generation(
        &self,
        session_id: &SessionId,
        generation: i64,
    ) -> StorageResult<Option<StoredGeneration>>;

    /// 按 session 和 sequence 读取可恢复事件。
    fn events_since(&self, query: &EventQuery) -> StorageResult<Vec<StoredDaemonEvent>>;

    /// 按 Turn 读取可恢复事件。
    fn events_for_turn(
        &self,
        turn_id: &TurnId,
        after_sequence: EventSequence,
    ) -> StorageResult<Vec<StoredDaemonEvent>>;

    /// 读取会话当前最大的持久化事件序号。
    fn latest_event_sequence(&self, session_id: &SessionId)
    -> StorageResult<Option<EventSequence>>;
}
