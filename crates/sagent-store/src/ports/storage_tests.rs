//! Storage 业务外观的 fake/recording 契约测试。

use sagent_types::{
    EventSequence, MessageId, SearchHit, SessionId, SessionSummary, StoredMessage, TurnId,
};

use super::{
    ReadOnlySessionStorage, ReadStorage, SearchStorage, SessionQueryStorage, SessionStorage,
    SessionWriteStorage, Storage, StorageResult, WriteOnlySessionStorage, WriteStorage,
};
use crate::{
    EventQuery, MessageQuery, MessageSearchQuery, MessageWindow, NewDaemonEvent, NewGeneration,
    NewMessage, NewSession, RestoreResult, RewindResult, SessionListQuery, StartTurn,
    StoredDaemonEvent, StoredGeneration, StoredRunningTurn,
};

/// 不连接数据库的写入 recording；所有调用都失败，从而测试只验证能力装配。
struct RecordingWrite;

impl SessionWriteStorage for RecordingWrite {
    fn create_session(&mut self, _: &NewSession) -> StorageResult<()> {
        anyhow::bail!("recording write")
    }

    fn update_session_title(
        &mut self,
        _: &SessionId,
        _: Option<&str>,
        _: &str,
    ) -> StorageResult<bool> {
        anyhow::bail!("recording write")
    }

    fn finish_session(&mut self, _: &SessionId, _: &str, _: &str) -> StorageResult<bool> {
        anyhow::bail!("recording write")
    }

    fn set_session_archived(&mut self, _: &SessionId, _: bool, _: &str) -> StorageResult<bool> {
        anyhow::bail!("recording write")
    }

    fn rewind_to_message(
        &mut self,
        _: &SessionId,
        _: MessageId,
        _: &str,
    ) -> StorageResult<RewindResult> {
        anyhow::bail!("recording write")
    }

    fn restore_rewound_from(
        &mut self,
        _: &SessionId,
        _: MessageId,
        _: &str,
    ) -> StorageResult<RestoreResult> {
        anyhow::bail!("recording write")
    }

    fn create_generation(&mut self, _: &NewGeneration) -> StorageResult<()> {
        anyhow::bail!("recording write")
    }

    fn start_turn(&mut self, _: &StartTurn, _: &NewMessage) -> StorageResult<MessageId> {
        anyhow::bail!("recording write")
    }

    fn commit_assistant_tool_calls(
        &mut self,
        _: &TurnId,
        _: &NewMessage,
        _: &str,
    ) -> StorageResult<MessageId> {
        anyhow::bail!("recording write")
    }

    fn commit_tool_result(
        &mut self,
        _: &TurnId,
        _: &NewMessage,
        _: &str,
    ) -> StorageResult<MessageId> {
        anyhow::bail!("recording write")
    }

    fn complete_turn(&mut self, _: &TurnId, _: &NewMessage, _: &str) -> StorageResult<MessageId> {
        anyhow::bail!("recording write")
    }

    fn interrupt_turn(&mut self, _: &TurnId, _: &str, _: &str) -> StorageResult<()> {
        anyhow::bail!("recording write")
    }

    fn fail_turn(&mut self, _: &TurnId, _: &str, _: &str, _: &str) -> StorageResult<()> {
        anyhow::bail!("recording write")
    }

    fn append_event(&mut self, _: &NewDaemonEvent) -> StorageResult<EventSequence> {
        anyhow::bail!("recording write")
    }
}

/// 不连接数据库的查询 recording；所有调用都失败，从而测试只验证能力装配。
struct RecordingQuery;

impl SessionQueryStorage for RecordingQuery {
    fn list_sessions(&self, _: &SessionListQuery) -> StorageResult<Vec<SessionSummary>> {
        anyhow::bail!("recording query")
    }

    fn get_session(&self, _: &SessionId) -> StorageResult<Option<SessionSummary>> {
        anyhow::bail!("recording query")
    }

    fn get_messages_for_model(
        &self,
        _: &SessionId,
        _: &MessageQuery,
    ) -> StorageResult<Vec<StoredMessage>> {
        anyhow::bail!("recording query")
    }

    fn get_messages_for_display(
        &self,
        _: &SessionId,
        _: &MessageQuery,
    ) -> StorageResult<Vec<StoredMessage>> {
        anyhow::bail!("recording query")
    }

    fn get_messages_around(
        &self,
        _: &SessionId,
        _: MessageId,
        _: u32,
        _: bool,
    ) -> StorageResult<Option<MessageWindow>> {
        anyhow::bail!("recording query")
    }

    fn get_running_turn(&self, _: &SessionId) -> StorageResult<Option<StoredRunningTurn>> {
        anyhow::bail!("recording query")
    }

    fn get_generation(&self, _: &SessionId, _: i64) -> StorageResult<Option<StoredGeneration>> {
        anyhow::bail!("recording query")
    }

    fn events_since(&self, _: &EventQuery) -> StorageResult<Vec<StoredDaemonEvent>> {
        anyhow::bail!("recording query")
    }

    fn events_for_turn(
        &self,
        _: &TurnId,
        _: EventSequence,
    ) -> StorageResult<Vec<StoredDaemonEvent>> {
        anyhow::bail!("recording query")
    }

    fn latest_event_sequence(&self, _: &SessionId) -> StorageResult<Option<EventSequence>> {
        anyhow::bail!("recording query")
    }
}

/// 不连接数据库的搜索 recording，确保搜索能力可以单独注入只读聚合。
struct RecordingSearch;

impl SearchStorage for RecordingSearch {
    fn search_messages(&self, _: &MessageSearchQuery) -> StorageResult<Vec<SearchHit>> {
        anyhow::bail!("recording search")
    }
}

#[test]
fn business_storage_aggregates_accept_recording_ports_without_sqlite() {
    // Arrange：三类领域端口均为不触碰文件系统的 recording 实现。
    let storage = Storage::new(SessionStorage::new(
        RecordingWrite,
        ReadOnlySessionStorage::new(RecordingQuery, RecordingSearch),
    ));
    let read = ReadStorage::new(ReadOnlySessionStorage::new(RecordingQuery, RecordingSearch));
    let mut write = WriteStorage::new(WriteOnlySessionStorage::new(RecordingWrite));

    // Act/Assert：完整聚合按 storage.session 暴露业务方法；窄聚合只暴露对应能力。
    assert!(
        storage
            .session
            .list_sessions(&SessionListQuery::default())
            .is_err()
    );
    assert!(
        storage
            .session
            .search_messages(&MessageSearchQuery::new("term"))
            .is_err()
    );
    assert!(
        read.session
            .list_sessions(&SessionListQuery::default())
            .is_err()
    );
    assert!(
        read.session
            .search_messages(&MessageSearchQuery::new("term"))
            .is_err()
    );
    assert!(
        write
            .session
            .create_session(&NewSession {
                id: SessionId::new("recording"),
                source: None,
                model: None,
                title: None,
                started_at: "2026-09-13T00:00:00Z".to_owned(),
            })
            .is_err()
    );
}
