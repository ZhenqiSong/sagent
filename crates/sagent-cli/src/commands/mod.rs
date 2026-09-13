//! 按业务边界拆分的 CLI 命令实现。
//!
//! 作者：SongZQ

use std::{path::PathBuf, sync::OnceLock};

use anyhow::{Context, Result};
use clap::Subcommand;

use crate::output::OutputFormat;

pub mod profile;
pub mod session;

mod storage;

use storage::CliStorageContext;

/// 一次 CLI 调用共享的全局运行参数与按需装配存储。
///
/// Session handler 第一次真正需要持久化时才绑定 Profile 存储上下文，并在同一条命令
/// 中复用它；这样参数校验可以先于配置/后端 I/O，Profile 管理命令也不会误初始化存储。
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

/// 顶层命令分组。
///
/// 新增一个业务域只需在此添加一个分组变体；该域的子命令和执行逻辑均留在自己的模块。
#[derive(Debug, Subcommand)]
pub enum Command {
    Profile {
        /// Profile 子命令。
        #[command(subcommand)]
        command: profile::ProfileCommand,
    },
    Session {
        /// Session 子命令。
        #[command(subcommand)]
        command: session::SessionCommand,
    },
}

impl Command {
    /// 将根级运行上下文转交给所属业务域。
    pub fn execute(self, context: &CommandContext) -> Result<()> {
        match self {
            Self::Profile { command } => command.execute(context),
            Self::Session { command } => command.execute(context),
        }
    }
}
