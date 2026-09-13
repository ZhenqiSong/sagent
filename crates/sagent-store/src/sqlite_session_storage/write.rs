//! SQLite Session 写入端口。
//!
//! 本模块只把领域写操作映射到共享 SQLite 数据库句柄；事务边界和状态转换仍由
//! `SqliteDatabase` 的同主题操作保持，避免上层把一次原子变更拆成多次 CRUD。
//!
//! 作者：SongZQ

use sagent_types::{EventSequence, MessageId, SessionId, TurnId};

use super::{SharedDatabase, lock_database};
use crate::{
    NewDaemonEvent, NewGeneration, NewMessage, NewSession, RestoreResult, RewindResult, StartTurn,
    StorageResult, ports::SessionWriteStorage,
};

/// SQLite Session 写端口，仅持有受保护的数据库资源。
pub(super) struct SqliteSessionStorage {
    pub(super) database: SharedDatabase,
}

impl SessionWriteStorage for SqliteSessionStorage {
    fn create_session(&mut self, session: &NewSession) -> StorageResult<()> {
        lock_database(&self.database)?.create_session(session)
    }

    fn update_session_title(
        &mut self,
        session_id: &SessionId,
        title: Option<&str>,
        updated_at: &str,
    ) -> StorageResult<bool> {
        lock_database(&self.database)?.update_session_title(session_id, title, updated_at)
    }

    fn finish_session(
        &mut self,
        session_id: &SessionId,
        end_reason: &str,
        ended_at: &str,
    ) -> StorageResult<bool> {
        lock_database(&self.database)?.finish_session(session_id, end_reason, ended_at)
    }

    fn set_session_archived(
        &mut self,
        session_id: &SessionId,
        archived: bool,
        updated_at: &str,
    ) -> StorageResult<bool> {
        lock_database(&self.database)?.set_session_archived(session_id, archived, updated_at)
    }

    fn rewind_to_message(
        &mut self,
        session_id: &SessionId,
        target_message_id: MessageId,
        updated_at: &str,
    ) -> StorageResult<RewindResult> {
        lock_database(&self.database)?.rewind_to_message(session_id, target_message_id, updated_at)
    }

    fn restore_rewound_from(
        &mut self,
        session_id: &SessionId,
        target_message_id: MessageId,
        updated_at: &str,
    ) -> StorageResult<RestoreResult> {
        lock_database(&self.database)?.restore_rewound_from(
            session_id,
            target_message_id,
            updated_at,
        )
    }

    fn create_generation(&mut self, generation: &NewGeneration) -> StorageResult<()> {
        lock_database(&self.database)?.create_generation(generation)
    }

    fn start_turn(
        &mut self,
        turn: &StartTurn,
        user_message: &NewMessage,
    ) -> StorageResult<MessageId> {
        lock_database(&self.database)?.begin_turn(turn, user_message)
    }

    fn commit_assistant_tool_calls(
        &mut self,
        turn_id: &TurnId,
        message: &NewMessage,
        committed_at: &str,
    ) -> StorageResult<MessageId> {
        lock_database(&self.database)?.commit_assistant_tool_calls(turn_id, message, committed_at)
    }

    fn commit_tool_result(
        &mut self,
        turn_id: &TurnId,
        message: &NewMessage,
        completed_at: &str,
    ) -> StorageResult<MessageId> {
        lock_database(&self.database)?.commit_tool_result(turn_id, message, completed_at)
    }

    fn complete_turn(
        &mut self,
        turn_id: &TurnId,
        assistant_message: &NewMessage,
        completed_at: &str,
    ) -> StorageResult<MessageId> {
        lock_database(&self.database)?.complete_turn(turn_id, assistant_message, completed_at)
    }

    fn interrupt_turn(
        &mut self,
        turn_id: &TurnId,
        reason: &str,
        completed_at: &str,
    ) -> StorageResult<()> {
        lock_database(&self.database)?.interrupt_turn(turn_id, reason, completed_at)
    }

    fn fail_turn(
        &mut self,
        turn_id: &TurnId,
        category: &str,
        message: &str,
        completed_at: &str,
    ) -> StorageResult<()> {
        lock_database(&self.database)?.fail_turn(turn_id, category, message, completed_at)
    }

    fn append_event(&mut self, event: &NewDaemonEvent) -> StorageResult<EventSequence> {
        lock_database(&self.database)?.append_event(event)
    }
}
