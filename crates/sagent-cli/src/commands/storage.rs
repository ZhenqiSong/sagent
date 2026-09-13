//! CLI 使用的 Profile 存储装配边界。
//!
//! 本模块只负责读取一次已选 Profile 配置并把存储描述符绑定到当前可用的
//! `StorageFactory`。同一条 CLI 命令共享一个 `CliStorageContext`，各业务函数只能通过它
//! 申请所需的领域端口，不再重复解析配置或依赖 SQLite 连接细节；R3.5 的
//! `StorageManager` 将接替这里的具体 adapter 选择。

use std::{path::Path, sync::Arc};

use anyhow::{Context, Result};
use sagent_config::{
    SagentPaths, load_profile_config, normalize_profile_name, resolve_active_paths,
};
use sagent_store::{
    SqliteStorageFactory, StorageDependencies, StorageFactory, StorageReadDependencies,
};

/// 一条 CLI 命令作用域内的存储装配上下文。
///
/// Factory 只绑定已解析的 Profile 存储意图，不持有本次操作的领域端口；每个命令函数
/// 按职责申请一组新的只读或可写依赖，从而保留端口生命周期和写入隔离。后续替换为
/// `StorageManager` 时，只需调整本类型的内部实现。
#[derive(Clone)]
pub(crate) struct CliStorageContext {
    factory: Arc<dyn StorageFactory>,
}

impl CliStorageContext {
    /// 将已绑定的 Factory 放入当前命令上下文。
    fn from_factory(factory: Arc<dyn StorageFactory>) -> Self {
        Self { factory }
    }

    /// 为查询命令申请只读领域端口，不向调用方暴露写入能力。
    pub(crate) fn open_read(&self) -> Result<StorageReadDependencies> {
        self.factory
            .create_readonly()
            .context("打开当前 Profile 只读存储失败")
    }

    /// 为创建和生命周期命令申请可写领域端口。
    pub(crate) fn open_write(&self) -> Result<StorageDependencies> {
        self.factory
            .create()
            .context("打开当前 Profile 可写存储失败")
    }
}

/// 根据已解析的 Profile 配置创建 CLI 作用域内的存储工厂。
///
/// 当前 R3.4 只支持 SQLite，因此 selector 在 CLI 边界绑定 SQLite adapter；远程、只读
/// 策略和未实现的 descriptor 组合会在这里 fail-closed，不会静默回退到 `state.db`。
fn factory_from_paths(paths: &SagentPaths) -> Result<Arc<dyn StorageFactory>> {
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

/// 根据已解析的 Profile 路径创建一条 CLI 命令使用的存储上下文。
pub(crate) fn storage_from_paths(paths: &SagentPaths) -> Result<CliStorageContext> {
    Ok(CliStorageContext::from_factory(factory_from_paths(paths)?))
}

/// 根据 CLI 的 home/profile 选项解析路径并创建一条命令共享的存储上下文。
pub(crate) fn storage_from_options(
    home: Option<&Path>,
    profile_override: Option<&str>,
) -> Result<CliStorageContext> {
    let profile = profile_override.map(normalize_profile_name).transpose()?;
    let paths = resolve_active_paths(home, profile.as_ref())?;
    storage_from_paths(&paths)
}
