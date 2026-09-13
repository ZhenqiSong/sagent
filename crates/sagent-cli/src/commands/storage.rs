//! CLI 使用的 Profile 存储装配边界。
//!
//! 本模块只负责读取一次已选 Profile 配置并把存储描述符绑定到当前可用的
//! `StorageFactory`。查询和写入命令通过工厂申请领域端口，不再直接打开 `Store` 或依赖
//! SQLite 连接细节；R3.5 的 `StorageManager` 将接替这里的具体 adapter 选择。

use std::{path::Path, sync::Arc};

use anyhow::{Context, Result};
use sagent_config::{
    SagentPaths, load_profile_config, normalize_profile_name, resolve_active_paths,
};
use sagent_store::{SqliteStorageFactory, StorageFactory};

/// 根据已解析的 Profile 配置创建 CLI 作用域内的存储工厂。
///
/// 当前 R3.4 只支持 SQLite，因此 selector 在 CLI 边界绑定 SQLite adapter；远程、只读
/// 策略和未实现的 descriptor 组合会在这里 fail-closed，不会静默回退到 `state.db`。
pub(crate) fn factory_from_paths(paths: &SagentPaths) -> Result<Arc<dyn StorageFactory>> {
    let config = load_profile_config(paths).context("读取当前 Profile 配置失败")?;
    let descriptor = config.get_storage_descriptor();
    descriptor
        .ensure_legacy_bootstrap_supported()
        .context("当前 Profile 存储配置不可用")?;
    let database_path = descriptor
        .resolve_sqlite_database_path(paths)
        .context("解析当前 Profile 数据库路径失败")?;
    let factory = SqliteStorageFactory::new(database_path).context("创建 SQLite 存储工厂失败")?;
    Ok(Arc::new(factory))
}

/// 根据 CLI 的 home/profile 选项解析路径并创建对应的存储工厂。
pub(crate) fn factory_from_options(
    home: Option<&Path>,
    profile_override: Option<&str>,
) -> Result<Arc<dyn StorageFactory>> {
    let profile = profile_override.map(normalize_profile_name).transpose()?;
    let paths = resolve_active_paths(home, profile.as_ref())?;
    factory_from_paths(&paths)
}
