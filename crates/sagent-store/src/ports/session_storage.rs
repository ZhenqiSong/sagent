//! 完整 Session 业务存储外观。
//!
//! 该对象把写入、查询和搜索端口组合成一个按业务意图命名的接口。它只负责转发调用，
//! 不负责打开数据库或编排事务；跨表原子性由底层 `SessionWriteStorage` 保证。

use sagent_types::{
    EventSequence, MessageId, SearchHit, SessionId, SessionSummary, StoredMessage, TurnId,
};

use super::{ReadOnlySessionStorage, SessionWriteStorage, StorageResult};
use crate::{
    EventQuery, MessageQuery, MessageSearchQuery, MessageWindow, NewDaemonEvent, NewGeneration,
    NewMessage, NewSession, RestoreResult, RewindResult, SessionListQuery, StartTurn,
    StoredDaemonEvent, StoredGeneration, StoredRunningTurn,
};

/// 完整的 Session 业务存储外观，统一承载写入、查询和搜索。
///
/// 该对象不等同于某一张数据库表；跨表的 Turn、事件和消息变更仍由底层写端口以一个
/// 高层原子方法完成。这里的转发方法使用业务意图命名，隐藏底层端口拆分。
pub struct SessionStorage {
    write: Box<dyn SessionWriteStorage>,
    read: ReadOnlySessionStorage,
}

impl SessionStorage {
    /// 组合 Session 领域的写入能力和只读业务外观。
    ///
    /// 查询与搜索都由 `ReadOnlySessionStorage` 统一封装；这样完整 Session 外观只需要
    /// 依赖一个只读能力对象，不会把两个只读底层端口继续传播到 Manager 或上层服务。
    pub fn new<W>(write: W, read: ReadOnlySessionStorage) -> Self
    where
        W: SessionWriteStorage + 'static,
    {
        Self {
            write: Box::new(write),
            read,
        }
    }

    pub(super) fn into_parts(
        self,
    ) -> (
        Box<dyn SessionWriteStorage>,
        Box<dyn super::SessionQueryStorage>,
        Box<dyn super::SearchStorage>,
    ) {
        let (query, search) = self.read.into_parts();
        (self.write, query, search)
    }

    /// 创建一个空会话。
    pub fn create_session(&mut self, session: &NewSession) -> StorageResult<()> {
        self.write.create_session(session)
    }

    /// 修改会话标题并返回目标是否存在。
    pub fn update_session_title(
        &mut self,
        session_id: &SessionId,
        title: Option<&str>,
        updated_at: &str,
    ) -> StorageResult<bool> {
        self.write
            .update_session_title(session_id, title, updated_at)
    }

    /// 写入会话结束时间和原因并返回目标是否存在。
    pub fn finish_session(
        &mut self,
        session_id: &SessionId,
        end_reason: &str,
        ended_at: &str,
    ) -> StorageResult<bool> {
        self.write.finish_session(session_id, end_reason, ended_at)
    }

    /// 设置会话归档状态并返回目标是否存在。
    pub fn set_session_archived(
        &mut self,
        session_id: &SessionId,
        archived: bool,
        updated_at: &str,
    ) -> StorageResult<bool> {
        self.write
            .set_session_archived(session_id, archived, updated_at)
    }

    /// 回退指定用户消息及其后的活动分支。
    pub fn rewind_to_message(
        &mut self,
        session_id: &SessionId,
        target_message_id: MessageId,
        updated_at: &str,
    ) -> StorageResult<RewindResult> {
        self.write
            .rewind_to_message(session_id, target_message_id, updated_at)
    }

    /// 从回退起点恢复旧分支，并在出现新活动分支时失败关闭。
    pub fn restore_rewound_from(
        &mut self,
        session_id: &SessionId,
        target_message_id: MessageId,
        updated_at: &str,
    ) -> StorageResult<RestoreResult> {
        self.write
            .restore_rewound_from(session_id, target_message_id, updated_at)
    }

    /// 写入可复现的 generation 快照。
    pub fn create_generation(&mut self, generation: &NewGeneration) -> StorageResult<()> {
        self.write.create_generation(generation)
    }

    /// 原子写入用户消息、running Turn 和开始事件。
    pub fn start_turn(
        &mut self,
        turn: &StartTurn,
        user_message: &NewMessage,
    ) -> StorageResult<MessageId> {
        self.write.start_turn(turn, user_message)
    }

    /// 原子提交 assistant 工具调用消息。
    pub fn commit_assistant_tool_calls(
        &mut self,
        turn_id: &TurnId,
        message: &NewMessage,
        committed_at: &str,
    ) -> StorageResult<MessageId> {
        self.write
            .commit_assistant_tool_calls(turn_id, message, committed_at)
    }

    /// 原子提交工具结果消息。
    pub fn commit_tool_result(
        &mut self,
        turn_id: &TurnId,
        message: &NewMessage,
        completed_at: &str,
    ) -> StorageResult<MessageId> {
        self.write
            .commit_tool_result(turn_id, message, completed_at)
    }

    /// 原子提交最终 assistant 消息并完成 Turn。
    pub fn complete_turn(
        &mut self,
        turn_id: &TurnId,
        assistant_message: &NewMessage,
        completed_at: &str,
    ) -> StorageResult<MessageId> {
        self.write
            .complete_turn(turn_id, assistant_message, completed_at)
    }

    /// 原子将 running Turn 标记为 interrupted。
    pub fn interrupt_turn(
        &mut self,
        turn_id: &TurnId,
        reason: &str,
        completed_at: &str,
    ) -> StorageResult<()> {
        self.write.interrupt_turn(turn_id, reason, completed_at)
    }

    /// 原子将 running Turn 标记为 failed。
    pub fn fail_turn(
        &mut self,
        turn_id: &TurnId,
        category: &str,
        message: &str,
        completed_at: &str,
    ) -> StorageResult<()> {
        self.write
            .fail_turn(turn_id, category, message, completed_at)
    }

    /// 写入一个独立的 daemon event。
    pub fn append_event(&mut self, event: &NewDaemonEvent) -> StorageResult<EventSequence> {
        self.write.append_event(event)
    }

    /// 按可见性条件和分页读取会话摘要。
    pub fn list_sessions(&self, query: &SessionListQuery) -> StorageResult<Vec<SessionSummary>> {
        self.read.list_sessions(query)
    }

    /// 按完整 ID 读取会话摘要。
    pub fn get_session(&self, session_id: &SessionId) -> StorageResult<Option<SessionSummary>> {
        self.read.get_session(session_id)
    }

    /// 读取仅供模型使用的活动消息上下文。
    pub fn get_messages_for_model(
        &self,
        session_id: &SessionId,
        query: &MessageQuery,
    ) -> StorageResult<Vec<StoredMessage>> {
        self.read.get_messages_for_model(session_id, query)
    }

    /// 读取用户可见的会话消息历史。
    pub fn get_messages_for_display(
        &self,
        session_id: &SessionId,
        query: &MessageQuery,
    ) -> StorageResult<Vec<StoredMessage>> {
        self.read.get_messages_for_display(session_id, query)
    }

    /// 读取指定消息两侧的上下文窗口。
    pub fn get_messages_around(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        window: u32,
        include_inactive: bool,
    ) -> StorageResult<Option<MessageWindow>> {
        self.read
            .get_messages_around(session_id, message_id, window, include_inactive)
    }

    /// 读取会话中唯一的 running Turn。
    pub fn get_running_turn(
        &self,
        session_id: &SessionId,
    ) -> StorageResult<Option<StoredRunningTurn>> {
        self.read.get_running_turn(session_id)
    }

    /// 读取指定 generation 快照。
    pub fn get_generation(
        &self,
        session_id: &SessionId,
        generation: i64,
    ) -> StorageResult<Option<StoredGeneration>> {
        self.read.get_generation(session_id, generation)
    }

    /// 按 Session 和 sequence 读取可恢复事件。
    pub fn events_since(&self, query: &EventQuery) -> StorageResult<Vec<StoredDaemonEvent>> {
        self.read.events_since(query)
    }

    /// 按 Turn 读取可恢复事件。
    pub fn events_for_turn(
        &self,
        turn_id: &TurnId,
        after_sequence: EventSequence,
    ) -> StorageResult<Vec<StoredDaemonEvent>> {
        self.read.events_for_turn(turn_id, after_sequence)
    }

    /// 读取会话当前最大的持久化事件序号。
    pub fn latest_event_sequence(
        &self,
        session_id: &SessionId,
    ) -> StorageResult<Option<EventSequence>> {
        self.read.latest_event_sequence(session_id)
    }

    /// 执行有界消息全文搜索。
    pub fn search_messages(&self, query: &MessageSearchQuery) -> StorageResult<Vec<SearchHit>> {
        self.read.search_messages(query)
    }
}
