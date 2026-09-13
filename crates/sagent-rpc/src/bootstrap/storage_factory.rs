//! 根据 Profile 存储意图选择具体 StorageFactory。
//!
//! 本模块是 Bootstrap 与后端 adapter 之间的唯一选择边界：它读取已解析的
//! `StorageDescriptor`，负责校验当前后端是否可用并构造对应 Factory；Runtime、RPC
//! service 和工具只接收抽象 `StorageFactory`，不会根据 `StorageKind` 分支。

use std::sync::Arc;

use anyhow::{Result, bail};
use sagent_config::{SagentPaths, StorageDescriptor, StorageKind};
use sagent_store::{SqliteStorageFactory, StorageFactory};

/// 根据已解析的存储配置创建后端 Factory。
///
/// 输入是一次配置快照中的 descriptor 和 Profile 路径上下文，不会重新读取文件或读取
/// 远程密钥。Factory 的连接、事务和 migration 生命周期仍由具体 adapter 管理；未实现
/// 的后端在这里明确失败，禁止静默回退到 SQLite。
pub(crate) fn create_storage_factory(
    paths: &SagentPaths,
    descriptor: &StorageDescriptor,
) -> Result<Arc<dyn StorageFactory>> {
    descriptor.validate()?;
    match descriptor.kind {
        StorageKind::Sqlite => {
            if descriptor.schema.is_some() || descriptor.namespace.is_some() || descriptor.read_only
            {
                bail!("当前 SQLite storage 不支持 schema、namespace 或只读策略")
            }
            let database_path = descriptor.resolve_sqlite_database_path(paths)?;
            Ok(Arc::new(SqliteStorageFactory::new(database_path)?))
        }
        StorageKind::Remote => bail!("remote storage 后端尚未实现"),
    }
}

#[cfg(test)]
mod tests {
    use sagent_config::{SagentPaths, StorageDescriptor, StorageKind};

    use super::create_storage_factory;

    fn paths() -> SagentPaths {
        let root = std::env::temp_dir().join("sagent-storage-selector-test");
        SagentPaths {
            profile: "default".to_owned(),
            config_yaml: root.join("config.yaml"),
            env_file: root.join(".env"),
            sagent_home: root,
        }
    }

    #[test]
    fn selects_sqlite_factory_without_exposing_its_type() {
        assert!(
            create_storage_factory(&paths(), &StorageDescriptor::default()).is_ok(),
            "默认 SQLite descriptor 应能选择 Factory"
        );
    }

    #[test]
    fn rejects_remote_before_any_sqlite_path_is_resolved() {
        let descriptor = StorageDescriptor {
            kind: StorageKind::Remote,
            connection_env: Some("SAGENT_DB_URL".to_owned()),
            ..StorageDescriptor::default()
        };
        let result = create_storage_factory(&paths(), &descriptor);
        assert!(result.is_err(), "未实现的远程后端必须明确失败");
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
        let result = create_storage_factory(&paths(), &descriptor);
        assert!(result.is_err(), "SQLite 未实现的 schema 必须 fail-closed");
    }
}
