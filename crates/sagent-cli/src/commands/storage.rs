//! CLI 使用的 Profile 存储装配边界。
//!
//! 本模块只负责读取一次已选 Profile 配置并把存储描述符绑定到当前可用的
//! `StorageManager`。同一条 CLI 命令共享一个 `CliStorageContext`，各业务函数只能通过它
//! 申请所需的业务存储，不再重复解析配置或依赖 SQLite 连接细节。

use std::{path::Path, sync::Arc};

use anyhow::{Context, Result};
use sagent_config::{
    SagentPaths, load_profile_config, normalize_profile_name, resolve_active_paths,
};
use sagent_store::{ReadStorage, StorageManager, WriteStorage, create_storage_manager};

/// 一条 CLI 命令作用域内的存储装配上下文。
///
/// Manager 只绑定已解析的 Profile 存储意图，不持有本次操作的业务存储；每个命令函数
/// 按职责申请新的只读或可写对象，从而保留端口生命周期和写入隔离。
#[derive(Clone)]
pub(crate) struct CliStorageContext {
    manager: Arc<dyn StorageManager>,
}

impl CliStorageContext {
    /// 将已绑定的 Manager 放入当前命令上下文。
    fn from_manager(manager: Arc<dyn StorageManager>) -> Self {
        Self { manager }
    }

    /// 为查询命令申请只读领域端口，不向调用方暴露写入能力。
    pub(crate) fn open_read(&self) -> Result<ReadStorage> {
        self.manager
            .open_read_storage()
            .context("打开当前 Profile 只读存储失败")
    }

    /// 为创建和生命周期命令申请可写领域端口。
    pub(crate) fn open_write(&self) -> Result<WriteStorage> {
        self.manager
            .open_write_storage()
            .context("打开当前 Profile 可写存储失败")
    }

    /// 为 Profile 创建等显式初始化流程执行 migration。
    pub(crate) fn initialize(&self) -> Result<()> {
        self.manager
            .initialize()
            .context("初始化当前 Profile 存储失败")
    }
}

/// 根据已解析的 Profile 配置创建 CLI 作用域内的存储 Manager。
///
/// 当前后端能力只支持 SQLite；远程、只读策略和未实现的 descriptor 组合由共享 selector
/// fail-closed，不会静默回退到 `state.db`。
fn manager_from_paths(paths: &SagentPaths) -> Result<Arc<dyn StorageManager>> {
    let config = load_profile_config(paths).context("读取当前 Profile 配置失败")?;
    let descriptor = config.get_storage_descriptor();
    create_storage_manager(paths, descriptor).context("创建当前 Profile 存储管理器失败")
}

/// 根据已解析的 Profile 路径创建一条 CLI 命令使用的存储上下文。
pub(crate) fn storage_from_paths(paths: &SagentPaths) -> Result<CliStorageContext> {
    Ok(CliStorageContext::from_manager(manager_from_paths(paths)?))
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
