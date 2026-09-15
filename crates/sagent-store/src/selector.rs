//! 根据 Profile 存储意图选择具体 `StorageManager`。
//!
//! selector 是 CLI、RPC 和其它 bootstrap 入口共享的唯一后端选择边界。它只接收已经
//! 解析的 `StorageDescriptor`，不读取配置文件；具体连接、migration 和领域端口装配
//! 仍由选中的 Manager/adapter 负责。

use std::{path::PathBuf, sync::Arc};

use anyhow::{Result, bail};
use sagent_config::{SagentPaths, StorageDescriptor, StorageKind};

use crate::{SqliteStorageManager, StorageManager};

/// 根据已解析的存储配置创建后端 Manager。
///
/// `paths` 只用于把 SQLite 的相对路径或默认文件名解析成 Profile 作用域的绝对路径；
/// descriptor 已由配置快照提供，因此本函数不会重新读取 `config.yaml`。未实现的后端
/// 或当前 adapter 不支持的选项会明确失败，禁止静默回退到 SQLite。
pub fn create_storage_manager(
    paths: &SagentPaths,
    descriptor: &StorageDescriptor,
) -> Result<Arc<dyn StorageManager>> {
    let database_path = resolve_sqlite_database_path(paths, descriptor)?;
    Ok(Arc::new(SqliteStorageManager::new(database_path)?))
}

/// 校验当前 SQLite adapter 的能力并解析数据库路径。
///
/// Remote 会在路径计算前失败，不会意外创建本地 `state.db`。
fn resolve_sqlite_database_path(
    paths: &SagentPaths,
    descriptor: &StorageDescriptor,
) -> Result<PathBuf> {
    descriptor.validate()?;
    match descriptor.kind {
        StorageKind::Sqlite => {
            if descriptor.schema.is_some() || descriptor.namespace.is_some() || descriptor.read_only
            {
                bail!("当前 SQLite storage 不支持 schema、namespace 或只读策略")
            }
            descriptor.resolve_sqlite_database_path(paths)
        }
        StorageKind::Remote => bail!("remote storage 后端尚未实现"),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sagent_config::{SagentPaths, StorageDescriptor, StorageKind};

    use super::create_storage_manager;

    fn paths(name: &str) -> SagentPaths {
        let root = std::env::temp_dir().join(format!(
            "sagent-storage-selector-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        SagentPaths {
            profile: "default".to_owned(),
            config_yaml: root.join("config.yaml"),
            env_file: root.join(".env"),
            sagent_home: root,
        }
    }

    #[test]
    fn selects_sqlite_manager_without_opening_database() {
        let paths = paths("manager");
        let manager = create_storage_manager(&paths, &StorageDescriptor::default())
            .expect("默认 SQLite descriptor 应能选择 Manager");
        assert!(!paths.sagent_home.join("state.db").exists());
        manager
            .initialize()
            .expect("选出的 Manager 应能初始化 SQLite 数据库");
    }

    #[test]
    fn rejects_remote_before_any_sqlite_path_is_resolved() {
        let descriptor = StorageDescriptor {
            kind: StorageKind::Remote,
            connection_env: Some("SAGENT_DB_URL".to_owned()),
            ..StorageDescriptor::default()
        };
        let paths = paths("remote");
        let result = create_storage_manager(&paths, &descriptor);
        assert!(result.is_err());
        assert!(
            result
                .err()
                .expect("错误结果应包含失败原因")
                .to_string()
                .contains("remote storage")
        );
    }

    #[test]
    fn rejects_sqlite_options_not_supported_by_the_current_adapter() {
        let descriptor = StorageDescriptor {
            schema: Some("app".to_owned()),
            ..StorageDescriptor::default()
        };
        assert!(create_storage_manager(&paths("options"), &descriptor).is_err());
    }
}
