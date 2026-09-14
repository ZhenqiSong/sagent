//! 根据 Profile 存储意图选择具体 StorageManager。
//!
//! 本模块是 Bootstrap 与后端 adapter 之间的唯一选择边界：它读取已解析的
//! `StorageDescriptor`，负责校验当前后端是否可用并构造对应 Manager。兼容期仍保留
//! 兼容期仍保留 Factory 构造函数，但 Runtime、RPC service 和工具的生产路径统一接收
//! 抽象 `StorageManager`，不在调用方根据 `StorageKind` 分支。

use std::{path::PathBuf, sync::Arc};

use anyhow::{Result, bail};
use sagent_config::{SagentPaths, StorageDescriptor, StorageKind};
use sagent_store::{SqliteStorageFactory, SqliteStorageManager, StorageFactory, StorageManager};

/// 根据已解析的存储配置创建后端 Manager。
///
/// `paths` 只用于把 SQLite 的相对路径或默认文件名解析成 Profile 作用域的绝对路径；
/// descriptor 已由配置快照提供，因此本函数不会重新读取 `config.yaml`。Manager 的
/// 初始化、连接生命周期和领域端口装配由具体 adapter 负责；未实现的后端在这里明确
/// 失败，禁止静默回退到 SQLite。
pub(crate) fn create_storage_manager(
    paths: &SagentPaths,
    descriptor: &StorageDescriptor,
) -> Result<Arc<dyn StorageManager>> {
    let database_path = resolve_sqlite_database_path(paths, descriptor)?;
    Ok(Arc::new(SqliteStorageManager::new(database_path)?))
}

/// 根据已解析的存储配置创建后端 Factory。
///
/// 输入是一次配置快照中的 descriptor 和 Profile 路径上下文，不会重新读取文件或读取
/// 远程密钥。Factory 的连接、事务和 migration 生命周期仍由具体 adapter 管理；未实现
/// 的后端在这里明确失败，禁止静默回退到 SQLite。
// 迁移期保留给旧测试和外部适配验证；实际 RuntimeBootstrap 已统一走 Manager selector。
#[allow(dead_code)]
pub(crate) fn create_storage_factory(
    paths: &SagentPaths,
    descriptor: &StorageDescriptor,
) -> Result<Arc<dyn StorageFactory>> {
    let database_path = resolve_sqlite_database_path(paths, descriptor)?;
    Ok(Arc::new(SqliteStorageFactory::new(database_path)?))
}

/// 校验当前 SQLite adapter 的能力并解析数据库路径。
///
/// 该共享步骤保证 Manager 和迁移期 Factory 对同一个 descriptor 采用完全一致的
/// fail-closed 规则；Remote 会在路径计算前失败，不会意外创建本地 `state.db`。
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

    use super::{create_storage_factory, create_storage_manager};

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
    fn selects_sqlite_factory_without_exposing_its_type() {
        assert!(
            create_storage_factory(&paths("factory"), &StorageDescriptor::default()).is_ok(),
            "默认 SQLite descriptor 应能选择 Factory"
        );
    }

    #[test]
    fn selects_sqlite_manager_without_opening_database() {
        let paths = paths("manager");
        let manager = create_storage_manager(&paths, &StorageDescriptor::default())
            .expect("默认 SQLite descriptor 应能选择 Manager");
        assert!(
            !paths.sagent_home.join("state.db").exists(),
            "选择 Manager 不应提前创建 SQLite 文件"
        );
        // selector 只装配 Manager；显式 initialize 才允许创建数据库和执行 migration。
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
        let result = create_storage_factory(&paths, &descriptor);
        assert!(result.is_err(), "未实现的远程后端必须明确失败");
        assert!(
            result
                .err()
                .expect("错误结果应包含失败原因")
                .to_string()
                .contains("remote storage")
        );

        let manager_result = create_storage_manager(&paths, &descriptor);
        assert!(
            manager_result.is_err(),
            "未实现的远程后端必须拒绝创建 Manager"
        );
    }

    #[test]
    fn rejects_sqlite_options_not_supported_by_the_current_adapter() {
        let descriptor = StorageDescriptor {
            schema: Some("app".to_owned()),
            ..StorageDescriptor::default()
        };
        let result = create_storage_factory(&paths("options"), &descriptor);
        assert!(result.is_err(), "SQLite 未实现的 schema 必须 fail-closed");
    }
}
