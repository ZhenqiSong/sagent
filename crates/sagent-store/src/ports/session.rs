//! 会话和 Turn 的写入端口。

use sagent_types::{EventSequence, MessageId, SessionId, TurnId};

use crate::{
    NewDaemonEvent, NewGeneration, NewMessage, NewSession, RestoreResult, RewindResult, StartTurn,
};

use super::StorageResult;

/// 会话及 Turn 的最小写入端口。
///
/// 每个改变状态的方法都表达一个完整的业务操作。尤其是 Turn 相关方法必须由实现
/// 在一次原子边界内完成消息、状态和事件写入，调用方不能将它们拆成数据库步骤。
pub trait SessionWriteStorage: Send {
    /// 创建一个空会话。
    fn create_session(&mut self, session: &NewSession) -> StorageResult<()>;

    /// 修改会话标题；返回值表示目标会话是否存在。
    fn update_session_title(
        &mut self,
        session_id: &SessionId,
        title: Option<&str>,
        updated_at: &str,
    ) -> StorageResult<bool>;

    /// 写入会话结束时间和原因；返回值表示目标会话是否存在。
    fn finish_session(
        &mut self,
        session_id: &SessionId,
        end_reason: &str,
        ended_at: &str,
    ) -> StorageResult<bool>;

    /// 设置会话是否归档；返回值表示目标会话是否存在。
    fn set_session_archived(
        &mut self,
        session_id: &SessionId,
        archived: bool,
        updated_at: &str,
    ) -> StorageResult<bool>;

    /// 回退指定 user 消息及其后的活动分支，并返回恢复检查点。
    fn rewind_to_message(
        &mut self,
        session_id: &SessionId,
        target_message_id: MessageId,
        updated_at: &str,
    ) -> StorageResult<RewindResult>;

    /// 按回退起点恢复旧分支；发现新活动分支时必须失败关闭。
    fn restore_rewound_from(
        &mut self,
        session_id: &SessionId,
        target_message_id: MessageId,
        updated_at: &str,
    ) -> StorageResult<RestoreResult>;

    /// 写入一个可复现的 generation 快照。
    fn create_generation(&mut self, generation: &NewGeneration) -> StorageResult<()>;

    /// 原子写入用户消息、running Turn 和开始事件。
    ///
    /// 领域端口使用 `start_turn` 命名；SQLite adapter 可将其映射到现有的 `begin_turn`。
    fn start_turn(
        &mut self,
        turn: &StartTurn,
        user_message: &NewMessage,
    ) -> StorageResult<MessageId>;

    /// 原子提交 assistant 的工具调用消息。
    fn commit_assistant_tool_calls(
        &mut self,
        turn_id: &TurnId,
        message: &NewMessage,
        committed_at: &str,
    ) -> StorageResult<MessageId>;

    /// 原子提交工具结果消息。
    fn commit_tool_result(
        &mut self,
        turn_id: &TurnId,
        message: &NewMessage,
        completed_at: &str,
    ) -> StorageResult<MessageId>;

    /// 原子提交最终 assistant 消息并完成 Turn。
    fn complete_turn(
        &mut self,
        turn_id: &TurnId,
        assistant_message: &NewMessage,
        completed_at: &str,
    ) -> StorageResult<MessageId>;

    /// 原子将 running Turn 标记为 interrupted。
    fn interrupt_turn(
        &mut self,
        turn_id: &TurnId,
        reason: &str,
        completed_at: &str,
    ) -> StorageResult<()>;

    /// 原子将 running Turn 标记为 failed。
    fn fail_turn(
        &mut self,
        turn_id: &TurnId,
        category: &str,
        message: &str,
        completed_at: &str,
    ) -> StorageResult<()>;

    /// 写入一个独立的 daemon event。
    fn append_event(&mut self, event: &NewDaemonEvent) -> StorageResult<EventSequence>;
}
