//! 只写 Session 业务存储外观。
//!
//! 只写对象只组合 Session 写入端口，查询和搜索能力不会随对象传播。每个方法仍保持
//! 高层业务操作的原子边界，调用方不能将 Turn 状态与消息、事件拆成独立数据库步骤。

use sagent_types::{EventSequence, MessageId, SessionId, TurnId};

use super::{SessionWriteStorage, StorageResult};
use crate::{
    NewDaemonEvent, NewGeneration, NewMessage, NewSession, RestoreResult, RewindResult, StartTurn,
};

/// 只写 Session 业务外观，不包含查询或搜索能力。
pub struct WriteOnlySessionStorage {
    write: Box<dyn SessionWriteStorage>,
}

impl WriteOnlySessionStorage {
    /// 绑定会话写入端口。
    pub fn new<W>(write: W) -> Self
    where
        W: SessionWriteStorage + 'static,
    {
        Self {
            write: Box::new(write),
        }
    }

    pub(super) fn from_parts(write: Box<dyn SessionWriteStorage>) -> Self {
        Self { write }
    }

    pub(super) fn into_session(self) -> Box<dyn SessionWriteStorage> {
        self.write
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
}
