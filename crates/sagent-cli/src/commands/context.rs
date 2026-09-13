//! CLI 命令级运行上下文。
//!
//! 本模块只保存一次 CLI 调用共享的路径、Profile、输出格式和惰性存储装配；不负责命令
//! 分发或具体业务操作。
//!
//! 作者：SongZQ

use std::{path::PathBuf, sync::OnceLock};

use anyhow::{Context, Result};

use super::{storage, storage::CliStorageContext};
use crate::output::OutputFormat;

/// 一次 CLI 调用共享的全局运行参数与按需装配存储。
///
/// Context 只在第一次真正需要持久化时创建 `CliStorageContext`，从而让 Profile 管理等
/// 不涉及存储的命令不会提前读取配置或打开数据库；具体命令编排由领域 handler 负责。
pub struct CommandContext {
    /// 用户显式指定的数据目录。
    pub home: Option<PathBuf>,
    /// 用户显式指定的 Profile 名称；用于惰性存储装配。
    pub profile: Option<String>,
    /// 本次命令的输出格式。
    pub format: OutputFormat,
    /// 当前命令作用域内惰性初始化且共享的存储装配。
    storage: OnceLock<CliStorageContext>,
}

impl CommandContext {
    /// 创建尚未初始化存储的命令上下文。
    pub(crate) fn new(
        home: Option<PathBuf>,
        profile: Option<String>,
        format: OutputFormat,
    ) -> Self {
        Self {
            home,
            profile,
            format,
            storage: OnceLock::new(),
        }
    }

    /// 取得当前命令共享的存储上下文，并在首次调用时完成一次 Factory 装配。
    pub(crate) fn storage(&self) -> Result<&CliStorageContext> {
        if let Some(storage) = self.storage.get() {
            return Ok(storage);
        }

        let storage = storage::storage_from_options(self.home.as_deref(), self.profile.as_deref())?;
        // CLI 命令是同步串行执行的；忽略并发调用下的重复 set 结果后，统一读取已保存实例。
        let _ = self.storage.set(storage);
        self.storage.get().context("当前命令存储上下文初始化失败")
    }
}
