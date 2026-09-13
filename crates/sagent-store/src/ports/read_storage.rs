//! 只读 Session 业务存储外观。
//!
//! 只读对象只组合查询和搜索端口，类型上不存在任何 `&mut` 写入入口。后端可以因此
//! 使用只读连接或只读事务，而不会因为上层代码误调用而执行 migration 或修改数据。

use sagent_types::{
    EventSequence, MessageId, SearchHit, SessionId, SessionSummary, StoredMessage, TurnId,
};

use super::{SearchStorage, SessionQueryStorage, StorageResult};
use crate::{
    EventQuery, MessageQuery, MessageSearchQuery, MessageWindow, SessionListQuery,
    StoredDaemonEvent, StoredGeneration, StoredRunningTurn,
};

/// 只读 Session 业务外观，不包含任何写入能力。
pub struct ReadOnlySessionStorage {
    query: Box<dyn SessionQueryStorage>,
    search: Box<dyn SearchStorage>,
}

impl ReadOnlySessionStorage {
    /// 组合只读查询和搜索端口。
    pub fn new<Q, H>(query: Q, search: H) -> Self
    where
        Q: SessionQueryStorage + 'static,
        H: SearchStorage + 'static,
    {
        Self {
            query: Box::new(query),
            search: Box::new(search),
        }
    }

    pub(super) fn from_parts(
        query: Box<dyn SessionQueryStorage>,
        search: Box<dyn SearchStorage>,
    ) -> Self {
        Self { query, search }
    }

    pub(super) fn into_parts(self) -> (Box<dyn SessionQueryStorage>, Box<dyn SearchStorage>) {
        (self.query, self.search)
    }

    /// 按可见性条件和分页读取会话摘要。
    pub fn list_sessions(&self, query: &SessionListQuery) -> StorageResult<Vec<SessionSummary>> {
        self.query.list_sessions(query)
    }

    /// 按完整 ID 读取会话摘要。
    pub fn get_session(&self, session_id: &SessionId) -> StorageResult<Option<SessionSummary>> {
        self.query.get_session(session_id)
    }

    /// 读取仅供模型使用的活动消息上下文。
    pub fn get_messages_for_model(
        &self,
        session_id: &SessionId,
        query: &MessageQuery,
    ) -> StorageResult<Vec<StoredMessage>> {
        self.query.get_messages_for_model(session_id, query)
    }

    /// 读取用户可见的会话消息历史。
    pub fn get_messages_for_display(
        &self,
        session_id: &SessionId,
        query: &MessageQuery,
    ) -> StorageResult<Vec<StoredMessage>> {
        self.query.get_messages_for_display(session_id, query)
    }

    /// 读取指定消息两侧的上下文窗口。
    pub fn get_messages_around(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        window: u32,
        include_inactive: bool,
    ) -> StorageResult<Option<MessageWindow>> {
        self.query
            .get_messages_around(session_id, message_id, window, include_inactive)
    }

    /// 读取会话中唯一的 running Turn。
    pub fn get_running_turn(
        &self,
        session_id: &SessionId,
    ) -> StorageResult<Option<StoredRunningTurn>> {
        self.query.get_running_turn(session_id)
    }

    /// 读取指定 generation 快照。
    pub fn get_generation(
        &self,
        session_id: &SessionId,
        generation: i64,
    ) -> StorageResult<Option<StoredGeneration>> {
        self.query.get_generation(session_id, generation)
    }

    /// 按 Session 和 sequence 读取可恢复事件。
    pub fn events_since(&self, query: &EventQuery) -> StorageResult<Vec<StoredDaemonEvent>> {
        self.query.events_since(query)
    }

    /// 按 Turn 读取可恢复事件。
    pub fn events_for_turn(
        &self,
        turn_id: &TurnId,
        after_sequence: EventSequence,
    ) -> StorageResult<Vec<StoredDaemonEvent>> {
        self.query.events_for_turn(turn_id, after_sequence)
    }

    /// 读取会话当前最大的持久化事件序号。
    pub fn latest_event_sequence(
        &self,
        session_id: &SessionId,
    ) -> StorageResult<Option<EventSequence>> {
        self.query.latest_event_sequence(session_id)
    }

    /// 执行有界消息全文搜索。
    pub fn search_messages(&self, query: &MessageSearchQuery) -> StorageResult<Vec<SearchHit>> {
        self.search.search_messages(query)
    }
}
