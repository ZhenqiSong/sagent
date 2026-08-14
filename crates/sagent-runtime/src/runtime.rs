//! Runtime 公开装配入口。
//!
//! `Runtime::open` 在返回前完成配置校验、数据库打开、PRAGMA 和 migration；调用者拿到的
//! `RuntimeHandle` 才能接受 Session 操作。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 6 Runtime 入口

use sagent_config::{Config, ConfigPaths};

use crate::error::RuntimeError;
use crate::supervisor::{RuntimeHandle, Supervisor};

/// Runtime 启动器。
pub struct Runtime;

impl Runtime {
    /// 使用 `SAGENT_HOME` 或平台默认路径打开 Runtime。
    pub fn open(config: Config) -> Result<RuntimeHandle, RuntimeError> {
        let paths =
            ConfigPaths::discover().map_err(|error| RuntimeError::Config(error.to_string()))?;
        Self::open_at(config, paths)
    }

    /// 使用显式 home 路径打开 Runtime，供测试和进程装配使用。
    pub fn open_at(config: Config, paths: ConfigPaths) -> Result<RuntimeHandle, RuntimeError> {
        Supervisor::open(config, paths)
    }
}
