//! SQLite 存储端口的首个具体 adapter。
//!
//! 本模块是唯一负责把具体 `Store` 映射到领域端口的地方。上层只能持有
//! `StorageFactory` 或 `StorageDependencies`，不能通过端口访问 SQLite 连接。

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use anyhow::{Context, Result, bail};
use sagent_types::{
    EventSequence, MessageId, SearchHit, SessionId, SessionSummary, StoredMessage, TurnId,
};

use crate::{
    EventQuery, MessageQuery, MessageSearchQuery, MessageWindow, NewDaemonEvent, NewGeneration,
    NewMessage, NewSession, SessionListQuery, StartTurn, Store, StoredDaemonEvent,
    StoredGeneration, StoredRunningTurn,
    ports::{
        SearchStorage, SessionQueryStorage, SessionStorage, StorageDependencies, StorageFactory,
        StorageResult,
    },
};

/// 使用同一个数据库文件创建独立领域存储端口的 SQLite Factory。
pub struct SqliteStorageFactory {
    database_path: PathBuf,
}

impl SqliteStorageFactory {
    /// 绑定一个 SQLite 数据库路径，但不创建文件或打开连接。
    pub fn new(database_path: impl Into<PathBuf>) -> Result<Self> {
        let database_path = database_path.into();
        if !database_path.is_absolute() {
            bail!("SQLite 数据库路径必须是绝对路径");
        }
        Ok(Self { database_path })
    }
}

impl StorageFactory for SqliteStorageFactory {
    /// 创建独立的 SQLite 写入、查询和搜索端口。
    ///
    /// 首次创建会打开读写 Store 并执行已有 migration；三个领域端口共享这一组受保护
    /// 的连接。每次调用都会重新打开 Store，避免 Actor 之间共享 SQLite 连接。
    fn create(&self) -> StorageResult<StorageDependencies> {
        let writable = Store::open_readwrite(&self.database_path).with_context(|| {
            format!("打开 SQLite 写入存储失败：{}", self.database_path.display())
        })?;
        writable
            .verify_connection()
            .context("检查 SQLite 写入存储失败")?;

        Ok(StorageDependencies::from(writable))
    }
}

type SharedStore = Arc<Mutex<Store>>;

/// 将一个 Store 包装为三个共享同一连接生命周期的领域端口。
fn storage_dependencies_from_store(store: Store) -> StorageDependencies {
    let shared = Arc::new(Mutex::new(store));
    StorageDependencies::new(
        SqliteSessionStorage {
            store: Arc::clone(&shared),
        },
        SqliteSessionQueryStorage {
            store: Arc::clone(&shared),
        },
        SqliteSearchStorage { store: shared },
    )
}

/// 将兼容期的具体 Store 转换为领域端口聚合。
///
/// 该转换只在 adapter 边界使用，便于 Runtime 测试继续注入 Store 工厂；生产代码
/// 应优先直接注入 `StorageFactory`，不再让上层依赖具体 Store。
impl From<Store> for StorageDependencies {
    fn from(store: Store) -> Self {
        storage_dependencies_from_store(store)
    }
}

/// 将 SQLite Store 的写入操作映射为 SessionStorage 端口。
struct SqliteSessionStorage {
    store: SharedStore,
}

impl SessionStorage for SqliteSessionStorage {
    fn create_session(&mut self, session: &NewSession) -> StorageResult<()> {
        lock_store(&self.store)?.create_session(session)
    }

    fn create_generation(&mut self, generation: &NewGeneration) -> StorageResult<()> {
        lock_store(&self.store)?.create_generation(generation)
    }

    fn start_turn(
        &mut self,
        turn: &StartTurn,
        user_message: &NewMessage,
    ) -> StorageResult<MessageId> {
        lock_store(&self.store)?.begin_turn(turn, user_message)
    }

    fn commit_assistant_tool_calls(
        &mut self,
        turn_id: &TurnId,
        message: &NewMessage,
        committed_at: &str,
    ) -> StorageResult<MessageId> {
        lock_store(&self.store)?.commit_assistant_tool_calls(turn_id, message, committed_at)
    }

    fn commit_tool_result(
        &mut self,
        turn_id: &TurnId,
        message: &NewMessage,
        completed_at: &str,
    ) -> StorageResult<MessageId> {
        lock_store(&self.store)?.commit_tool_result(turn_id, message, completed_at)
    }

    fn complete_turn(
        &mut self,
        turn_id: &TurnId,
        assistant_message: &NewMessage,
        completed_at: &str,
    ) -> StorageResult<MessageId> {
        lock_store(&self.store)?.complete_turn(turn_id, assistant_message, completed_at)
    }

    fn interrupt_turn(
        &mut self,
        turn_id: &TurnId,
        reason: &str,
        completed_at: &str,
    ) -> StorageResult<()> {
        lock_store(&self.store)?.interrupt_turn(turn_id, reason, completed_at)
    }

    fn fail_turn(
        &mut self,
        turn_id: &TurnId,
        category: &str,
        message: &str,
        completed_at: &str,
    ) -> StorageResult<()> {
        lock_store(&self.store)?.fail_turn(turn_id, category, message, completed_at)
    }

    fn append_event(&mut self, event: &NewDaemonEvent) -> StorageResult<EventSequence> {
        lock_store(&self.store)?.append_event(event)
    }
}

/// 将 SQLite Store 的只读操作映射为 SessionQueryStorage 端口。
///
/// 查询连接通过 Mutex 保护，因为 `rusqlite::Connection` 不能跨线程共享；锁只覆盖
/// 单次查询，不会跨越 Runtime 的 await 或业务回调。
struct SqliteSessionQueryStorage {
    store: SharedStore,
}

impl SessionQueryStorage for SqliteSessionQueryStorage {
    fn list_sessions(&self, query: &SessionListQuery) -> StorageResult<Vec<SessionSummary>> {
        lock_store(&self.store)?.list_sessions_with(query)
    }

    fn get_session(&self, session_id: &SessionId) -> StorageResult<Option<SessionSummary>> {
        lock_store(&self.store)?.get_session(session_id)
    }

    fn get_messages_for_model(
        &self,
        session_id: &SessionId,
        query: &MessageQuery,
    ) -> StorageResult<Vec<StoredMessage>> {
        lock_store(&self.store)?.get_messages_for_model(session_id, query)
    }

    fn get_messages_for_display(
        &self,
        session_id: &SessionId,
        query: &MessageQuery,
    ) -> StorageResult<Vec<StoredMessage>> {
        lock_store(&self.store)?.get_messages_for_display(session_id, query)
    }

    fn get_messages_around(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        window: u32,
        include_inactive: bool,
    ) -> StorageResult<Option<MessageWindow>> {
        lock_store(&self.store)?.get_messages_around(
            session_id,
            message_id,
            window,
            include_inactive,
        )
    }

    fn get_running_turn(&self, session_id: &SessionId) -> StorageResult<Option<StoredRunningTurn>> {
        lock_store(&self.store)?.get_running_turn(session_id)
    }

    fn get_generation(
        &self,
        session_id: &SessionId,
        generation: i64,
    ) -> StorageResult<Option<StoredGeneration>> {
        lock_store(&self.store)?.get_generation(session_id, generation)
    }

    fn events_since(&self, query: &EventQuery) -> StorageResult<Vec<StoredDaemonEvent>> {
        lock_store(&self.store)?.events_since(query)
    }

    fn events_for_turn(
        &self,
        turn_id: &TurnId,
        after_sequence: EventSequence,
    ) -> StorageResult<Vec<StoredDaemonEvent>> {
        lock_store(&self.store)?.events_for_turn(turn_id, after_sequence)
    }

    fn latest_event_sequence(
        &self,
        session_id: &SessionId,
    ) -> StorageResult<Option<EventSequence>> {
        lock_store(&self.store)?.latest_event_sequence(session_id)
    }
}

/// 将 SQLite FTS 查询映射为 SearchStorage 端口。
struct SqliteSearchStorage {
    store: SharedStore,
}

impl SearchStorage for SqliteSearchStorage {
    fn search_messages(&self, query: &MessageSearchQuery) -> StorageResult<Vec<SearchHit>> {
        lock_store(&self.store)?.search_messages(query)
    }
}

/// 将 Mutex 中毒转换成不泄漏实现细节的存储错误。
fn lock_store(store: &SharedStore) -> StorageResult<MutexGuard<'_, Store>> {
    store
        .lock()
        .map_err(|_| anyhow::anyhow!("SQLite 存储锁已中毒"))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::SqliteStorageFactory;
    use crate::{NewSession, StorageFactory};
    use sagent_types::SessionId;

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sagent-sqlite-factory-{name}-{}.db",
            std::process::id()
        ))
    }

    fn remove(path: &std::path::Path) {
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_relative_database_path_without_io() {
        assert!(SqliteStorageFactory::new("state.db").is_err());
    }

    #[test]
    fn creates_independent_domain_ports_over_sqlite() {
        let path = test_path("ports");
        remove(&path);
        let factory = SqliteStorageFactory::new(&path).expect("绝对路径应能绑定 Factory");
        let mut dependencies = factory.create().expect("Factory 应能创建端口");
        let session_id = SessionId::new("factory-session");

        dependencies
            .session_mut()
            .create_session(&NewSession {
                id: session_id.clone(),
                source: Some("test".to_owned()),
                model: Some("test-model".to_owned()),
                title: None,
                started_at: "2026-09-13T10:00:00Z".to_owned(),
            })
            .expect("写入端口应能创建会话");

        assert_eq!(
            dependencies
                .query()
                .get_session(&session_id)
                .expect("查询端口应能读取会话")
                .expect("刚创建的会话应存在")
                .id,
            session_id
        );

        let second = factory.create().expect("同一 Factory 应能创建第二组端口");
        assert!(
            second
                .query()
                .get_session(&session_id)
                .expect("第二组查询端口应能读取会话")
                .is_some()
        );
        remove(&path);
    }
}
