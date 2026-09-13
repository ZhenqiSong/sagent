//! SQLite 存储管理器。
//!
//! `SqliteStorageManager` 持有 SQLite 后端的不可变绑定信息，并实现通用
//! `StorageManager` 申请入口。它只负责连接打开、连接检查和端口聚合，不承担 Session
//! 或 Turn 的业务规则；这些规则仍由 `Store` 和领域端口实现维护。
//!
//! 作者：SongZQ

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::{StorageDependencies, StorageManager, StorageReadDependencies, StorageResult, Store};

/// 绑定单个 Profile SQLite 数据库的存储管理器。
///
/// 管理器只保存经过校验的绝对路径，不预先打开连接；每次申请依赖时由 SQLite adapter
/// 决定连接和事务的生命周期。这样可以先冻结统一 manager API，后续再替换为连接池而不
/// 改动上层领域端口。
pub struct SqliteStorageManager {
    database_path: PathBuf,
}

impl SqliteStorageManager {
    /// 创建绑定绝对 SQLite 路径的管理器，不创建数据库文件或执行 migration。
    pub fn new(database_path: impl Into<PathBuf>) -> Result<Self> {
        let database_path = database_path.into();
        if !database_path.is_absolute() {
            bail!("SQLite 数据库路径必须是绝对路径");
        }
        Ok(Self { database_path })
    }

    /// 以读写方式打开 Store，并验证连接可执行基本查询。
    fn open_writable_store(&self) -> Result<Store> {
        let store = Store::open_readwrite(&self.database_path).with_context(|| {
            format!("打开 SQLite 写入存储失败：{}", self.database_path.display())
        })?;
        store
            .verify_connection()
            .context("检查 SQLite 写入存储失败")?;
        Ok(store)
    }

    /// 以只读方式打开已有 Store，并验证连接可执行基本查询。
    fn open_readonly_store(&self) -> Result<Store> {
        let store = Store::open_readonly(&self.database_path).with_context(|| {
            format!("打开 SQLite 只读存储失败：{}", self.database_path.display())
        })?;
        store
            .verify_connection()
            .context("检查 SQLite 只读存储失败")?;
        Ok(store)
    }
}

impl StorageManager for SqliteStorageManager {
    /// 为 SessionActor 创建独立的 SQLite 领域端口集合。
    fn open_actor_storage(&self) -> StorageResult<StorageDependencies> {
        Ok(StorageDependencies::from(self.open_writable_store()?))
    }

    /// 为查询和搜索创建只读 SQLite 领域端口集合。
    fn open_read_storage(&self) -> StorageResult<StorageReadDependencies> {
        Ok(StorageReadDependencies::from(self.open_readonly_store()?))
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use sagent_types::SessionId;

    use super::SqliteStorageManager;
    use crate::{NewSession, StorageManager};

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sagent-sqlite-manager-{name}-{}.db",
            std::process::id()
        ))
    }

    fn remove(path: &std::path::Path) {
        let _ = fs::remove_file(path);
    }

    #[test]
    fn manager_provides_narrow_write_and_read_dependencies() {
        let path = test_path("narrow");
        remove(&path);
        let manager = SqliteStorageManager::new(&path).expect("绝对路径应能创建管理器");
        let session_id = SessionId::new("manager-session");

        let mut write = manager
            .open_write_storage()
            .expect("管理器应能提供可写依赖");
        write
            .session_mut()
            .create_session(&NewSession {
                id: session_id.clone(),
                source: Some("test".to_owned()),
                model: None,
                title: Some("管理器测试".to_owned()),
                started_at: "2026-09-13T10:00:00Z".to_owned(),
            })
            .expect("最小可写端口应能创建会话");
        drop(write);

        let read = manager.open_read_storage().expect("管理器应能提供只读依赖");
        assert!(
            read.query()
                .get_session(&session_id)
                .expect("只读查询应能读取会话")
                .is_some()
        );
        remove(&path);
    }

    #[test]
    fn readonly_manager_path_does_not_create_missing_database() {
        let path = test_path("readonly");
        remove(&path);
        let manager = SqliteStorageManager::new(&path).expect("绝对路径应能创建管理器");

        assert!(manager.open_read_storage().is_err());
        assert!(!path.exists(), "只读申请失败后不应创建数据库文件");
    }
}
