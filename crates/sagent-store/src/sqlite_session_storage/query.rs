//! SQLite Session 查询端口。
//!
//! 查询端口只在单次调用期间锁定数据库句柄，不跨越 Runtime 的 await 或业务回调；它不
//! 暴露写入方法，因此可安全装配到 `ReadStorage` 和完整 `Storage` 的只读部分。
//!
//! 作者：SongZQ

use sagent_types::{EventSequence, MessageId, SessionId, SessionSummary, StoredMessage, TurnId};

use super::{SharedDatabase, lock_database};
use crate::{
    EventQuery, MessageQuery, MessageWindow, SessionListQuery, StorageResult, StoredDaemonEvent,
    StoredGeneration, StoredRunningTurn, ports::SessionQueryStorage,
};

/// SQLite Session 查询端口，仅持有受保护的数据库资源。
pub(super) struct SqliteSessionQueryStorage {
    pub(super) database: SharedDatabase,
}

impl SessionQueryStorage for SqliteSessionQueryStorage {
    fn list_sessions(&self, query: &SessionListQuery) -> StorageResult<Vec<SessionSummary>> {
        lock_database(&self.database)?.list_sessions_with(query)
    }

    fn get_session(&self, session_id: &SessionId) -> StorageResult<Option<SessionSummary>> {
        lock_database(&self.database)?.get_session(session_id)
    }

    fn get_messages_for_model(
        &self,
        session_id: &SessionId,
        query: &MessageQuery,
    ) -> StorageResult<Vec<StoredMessage>> {
        lock_database(&self.database)?.get_messages_for_model(session_id, query)
    }

    fn get_messages_for_display(
        &self,
        session_id: &SessionId,
        query: &MessageQuery,
    ) -> StorageResult<Vec<StoredMessage>> {
        lock_database(&self.database)?.get_messages_for_display(session_id, query)
    }

    fn get_messages_around(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        window: u32,
        include_inactive: bool,
    ) -> StorageResult<Option<MessageWindow>> {
        lock_database(&self.database)?.get_messages_around(
            session_id,
            message_id,
            window,
            include_inactive,
        )
    }

    fn get_running_turn(&self, session_id: &SessionId) -> StorageResult<Option<StoredRunningTurn>> {
        lock_database(&self.database)?.get_running_turn(session_id)
    }

    fn get_generation(
        &self,
        session_id: &SessionId,
        generation: i64,
    ) -> StorageResult<Option<StoredGeneration>> {
        lock_database(&self.database)?.get_generation(session_id, generation)
    }

    fn events_since(&self, query: &EventQuery) -> StorageResult<Vec<StoredDaemonEvent>> {
        lock_database(&self.database)?.events_since(query)
    }

    fn events_for_turn(
        &self,
        turn_id: &TurnId,
        after_sequence: EventSequence,
    ) -> StorageResult<Vec<StoredDaemonEvent>> {
        lock_database(&self.database)?.events_for_turn(turn_id, after_sequence)
    }

    fn latest_event_sequence(
        &self,
        session_id: &SessionId,
    ) -> StorageResult<Option<EventSequence>> {
        lock_database(&self.database)?.latest_event_sequence(session_id)
    }
}
