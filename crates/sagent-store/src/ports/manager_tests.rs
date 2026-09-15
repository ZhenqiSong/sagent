//! `StorageManager` 的 recording 行为契约测试。
//!
//! recording manager 不连接数据库，只验证 Manager 每次申请都创建独立的业务存储、
//! 按访问职责记录申请，以及 Session 业务操作保持调用顺序。SQLite 的事务原子性由
//! adapter 的真实数据库契约测试覆盖，本模块不重复模拟 SQL。
//!
//! 作者：SongZQ
//! 创建日期：2026-09-15

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use sagent_types::{
    EventSequence, MessageId, SearchHit, SessionId, SessionSummary, StoredMessage, TurnId,
};

use super::{
    ReadOnlySessionStorage, ReadStorage, SearchStorage, SessionQueryStorage, SessionStorage,
    SessionWriteStorage, Storage, StorageManager, StorageResult, WriteOnlySessionStorage,
    WriteStorage,
};
use crate::{
    EventQuery, MessageQuery, MessageSearchQuery, MessageWindow, NewDaemonEvent, NewGeneration,
    NewMessage, NewSession, RestoreResult, RewindResult, SessionListQuery, StartTurn,
    StoredDaemonEvent, StoredGeneration, StoredRunningTurn,
};

#[derive(Clone, Default)]
struct Trace {
    events: Arc<Mutex<Vec<String>>>,
}

impl Trace {
    fn record(&self, event: impl Into<String>) {
        self.events
            .lock()
            .expect("recording trace lock should not be poisoned")
            .push(event.into());
    }

    fn snapshot(&self) -> Vec<String> {
        self.events
            .lock()
            .expect("recording trace lock should not be poisoned")
            .clone()
    }
}

struct RecordingManager {
    trace: Trace,
    next_actor: AtomicUsize,
    next_write: AtomicUsize,
}

impl RecordingManager {
    fn new() -> Self {
        Self {
            trace: Trace::default(),
            next_actor: AtomicUsize::new(0),
            next_write: AtomicUsize::new(0),
        }
    }

    fn write(&self, prefix: &str) -> ManagerWrite {
        let id = self.next_write.fetch_add(1, Ordering::SeqCst);
        ManagerWrite {
            label: format!("{prefix}:{id}"),
            trace: self.trace.clone(),
        }
    }
}

impl StorageManager for RecordingManager {
    fn open_actor_storage(&self) -> StorageResult<Storage> {
        let id = self.next_actor.fetch_add(1, Ordering::SeqCst);
        self.trace.record(format!("open_actor:{id}"));
        Ok(Storage::new(SessionStorage::new(
            ManagerWrite {
                label: format!("actor:{id}"),
                trace: self.trace.clone(),
            },
            ReadOnlySessionStorage::new(RecordingQuery, RecordingSearch),
        )))
    }

    fn open_read_storage(&self) -> StorageResult<ReadStorage> {
        self.trace.record("open_read");
        Ok(ReadStorage::new(ReadOnlySessionStorage::new(
            RecordingQuery,
            RecordingSearch,
        )))
    }

    fn open_write_storage(&self) -> StorageResult<WriteStorage> {
        self.trace.record("open_write");
        Ok(WriteStorage::new(WriteOnlySessionStorage::new(
            self.write("write"),
        )))
    }

    fn initialize(&self) -> StorageResult<()> {
        self.trace.record("initialize");
        Ok(())
    }

    fn health_check(&self) -> StorageResult<()> {
        self.trace.record("health_check");
        Ok(())
    }
}

struct ManagerWrite {
    label: String,
    trace: Trace,
}

impl ManagerWrite {
    fn reject<T>(&self, operation: &str) -> StorageResult<T> {
        self.trace.record(format!("{}:{operation}", self.label));
        anyhow::bail!("recording write")
    }
}

impl SessionWriteStorage for ManagerWrite {
    fn create_session(&mut self, _: &NewSession) -> StorageResult<()> {
        self.reject("create_session")
    }

    fn update_session_title(
        &mut self,
        _: &SessionId,
        _: Option<&str>,
        _: &str,
    ) -> StorageResult<bool> {
        self.reject("update_session_title")
    }

    fn finish_session(&mut self, _: &SessionId, _: &str, _: &str) -> StorageResult<bool> {
        self.reject("finish_session")
    }

    fn set_session_archived(&mut self, _: &SessionId, _: bool, _: &str) -> StorageResult<bool> {
        self.reject("set_session_archived")
    }

    fn rewind_to_message(
        &mut self,
        _: &SessionId,
        _: MessageId,
        _: &str,
    ) -> StorageResult<RewindResult> {
        self.reject("rewind_to_message")
    }

    fn restore_rewound_from(
        &mut self,
        _: &SessionId,
        _: MessageId,
        _: &str,
    ) -> StorageResult<RestoreResult> {
        self.reject("restore_rewound_from")
    }

    fn create_generation(&mut self, _: &NewGeneration) -> StorageResult<()> {
        self.reject("create_generation")
    }

    fn start_turn(&mut self, _: &StartTurn, _: &NewMessage) -> StorageResult<MessageId> {
        self.reject("start_turn")
    }

    fn commit_assistant_tool_calls(
        &mut self,
        _: &TurnId,
        _: &NewMessage,
        _: &str,
    ) -> StorageResult<MessageId> {
        self.reject("commit_assistant_tool_calls")
    }

    fn commit_tool_result(
        &mut self,
        _: &TurnId,
        _: &NewMessage,
        _: &str,
    ) -> StorageResult<MessageId> {
        self.reject("commit_tool_result")
    }

    fn complete_turn(&mut self, _: &TurnId, _: &NewMessage, _: &str) -> StorageResult<MessageId> {
        self.reject("complete_turn")
    }

    fn interrupt_turn(&mut self, _: &TurnId, _: &str, _: &str) -> StorageResult<()> {
        self.reject("interrupt_turn")
    }

    fn fail_turn(&mut self, _: &TurnId, _: &str, _: &str, _: &str) -> StorageResult<()> {
        self.reject("fail_turn")
    }

    fn append_event(&mut self, _: &NewDaemonEvent) -> StorageResult<EventSequence> {
        self.reject("append_event")
    }
}

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

struct RecordingSearch;

impl SearchStorage for RecordingSearch {
    fn search_messages(&self, _: &MessageSearchQuery) -> StorageResult<Vec<SearchHit>> {
        anyhow::bail!("recording search")
    }
}

fn sample_session() -> NewSession {
    NewSession {
        id: SessionId::new("manager-recording"),
        source: None,
        model: None,
        title: None,
        started_at: "2026-09-15T00:00:00Z".to_owned(),
    }
}

#[test]
fn manager_allocates_independent_actor_storage() {
    let manager = RecordingManager::new();
    let mut first = manager
        .open_actor_storage()
        .expect("recording manager 应能创建第一个 Actor 存储");
    let mut second = manager
        .open_actor_storage()
        .expect("recording manager 应能创建第二个 Actor 存储");

    assert!(first.session.create_session(&sample_session()).is_err());
    assert!(second.session.create_session(&sample_session()).is_err());
    assert_eq!(
        manager.trace.snapshot(),
        vec![
            "open_actor:0",
            "open_actor:1",
            "actor:0:create_session",
            "actor:1:create_session",
        ]
    );
}

#[test]
fn manager_preserves_session_operation_order_at_business_boundary() {
    let manager = RecordingManager::new();
    let mut storage = manager
        .open_actor_storage()
        .expect("recording manager 应能创建 Actor 存储");
    let session_id = SessionId::new("manager-order");
    let turn_id = TurnId::new();
    let start = StartTurn {
        turn_id,
        session_id: session_id.clone(),
        generation: 0,
        started_at: "2026-09-15T00:00:01Z".to_owned(),
    };
    let message = NewMessage::new(session_id, "user", "顺序", "2026-09-15T00:00:01Z");

    assert!(storage.session.start_turn(&start, &message).is_err());
    assert!(
        storage
            .session
            .commit_tool_result(&turn_id, &message, "2026-09-15T00:00:02Z")
            .is_err()
    );
    assert_eq!(
        manager.trace.snapshot(),
        vec![
            "open_actor:0",
            "actor:0:start_turn",
            "actor:0:commit_tool_result",
        ]
    );
}

#[test]
fn manager_records_narrow_access_and_lifecycle_entries() {
    let manager = RecordingManager::new();
    manager.initialize().expect("recording 初始化应成功");
    manager.health_check().expect("recording 健康检查应成功");
    let _read = manager
        .open_read_storage()
        .expect("recording manager 应能创建只读存储");
    let _write = manager
        .open_write_storage()
        .expect("recording manager 应能创建只写存储");

    assert_eq!(
        manager.trace.snapshot(),
        vec!["initialize", "health_check", "open_read", "open_write"]
    );
}
