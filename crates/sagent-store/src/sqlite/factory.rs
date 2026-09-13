//! SQLite 存储工厂的兼容构造边界。
//!
//! 本模块只负责把数据库路径绑定到 `SqliteStorageManager`，并保留旧
//! `StorageFactory` 接口的过渡实现。具体端口适配位于 `sqlite/session`，业务
//! 调用方不应通过 Factory 直接接触 SQLite 连接。
//!
//! 作者：SongZQ

use std::path::PathBuf;

use anyhow::Result;

use super::manager::SqliteStorageManager;
use crate::{
    StorageDependencies, StorageFactory, StorageManager, StorageReadDependencies, StorageResult,
};

/// 使用同一个数据库文件创建独立领域存储端口的 SQLite Factory。
pub struct SqliteStorageFactory {
    manager: SqliteStorageManager,
}

impl SqliteStorageFactory {
    /// 绑定一个 SQLite 数据库路径，但不创建文件或打开连接。
    pub fn new(database_path: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            manager: SqliteStorageManager::new(database_path)?,
        })
    }

    /// 将兼容期 Factory 转换为 Profile 作用域的存储管理器。
    ///
    /// 新的 bootstrap 应在选择后尽早调用此方法，使 Factory 只停留在构造边界；旧的
    /// `StorageFactory` 实现仍保留，便于现有调用方在 Manager 迁移期间继续工作。
    pub fn into_manager(self) -> SqliteStorageManager {
        self.manager
    }
}

impl StorageFactory for SqliteStorageFactory {
    /// 创建独立的 SQLite 写入、查询和搜索端口。
    ///
    /// 端口由 Manager 在边界内组装；这里仅拆出旧依赖结构，以保持迁移期调用方兼容。
    fn create(&self) -> StorageResult<StorageDependencies> {
        let storage = self.manager.open_actor_storage()?;
        let (session, query, search) = storage.into_parts();
        Ok(StorageDependencies::from_parts(session, query, search))
    }

    /// 创建只读 SQLite 查询与搜索端口，不执行 migration 或创建缺失数据库。
    fn create_readonly(&self) -> StorageResult<StorageReadDependencies> {
        let storage = self.manager.open_read_storage()?;
        let (query, search) = storage.into_parts();
        Ok(StorageReadDependencies::from_parts(query, search))
    }
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

    #[test]
    fn creates_readonly_domain_ports_without_creating_missing_database() {
        let path = test_path("readonly");
        remove(&path);
        let factory = SqliteStorageFactory::new(&path).expect("绝对路径应能绑定 Factory");

        assert!(
            factory.create_readonly().is_err(),
            "只读 Factory 不应创建缺失数据库"
        );
        assert!(!path.exists(), "只读打开失败后不应留下数据库文件");

        let mut writable = factory.create().expect("可写 Factory 应能初始化数据库");
        let session_id = SessionId::new("readonly-session");
        writable
            .session_mut()
            .create_session(&NewSession {
                id: session_id.clone(),
                source: Some("test".to_owned()),
                model: None,
                title: None,
                started_at: "2026-09-13T10:00:00Z".to_owned(),
            })
            .expect("可写端口应能创建会话");
        drop(writable);

        let readonly = factory
            .create_readonly()
            .expect("已有数据库应能创建只读端口");
        assert!(
            readonly
                .query()
                .get_session(&session_id)
                .expect("只读查询应能读取会话")
                .is_some()
        );
        remove(&path);
    }
}
