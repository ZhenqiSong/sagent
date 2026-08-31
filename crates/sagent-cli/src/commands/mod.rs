//! 按业务边界拆分的 CLI 命令实现。
//!
//! 作者：SongZQ

use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use crate::output::OutputFormat;

pub mod profile;
pub mod session;

/// 一次 CLI 调用共享的全局运行参数。
///
/// 命令实现通过此对象读取 home、profile 和输出格式，避免每新增一个子命令就扩展多个
/// `execute` 函数的参数列表。
#[derive(Clone, Debug)]
pub struct CommandContext {
    pub home: Option<PathBuf>,
    pub profile: Option<String>,
    pub format: OutputFormat,
}

/// 顶层命令分组。
///
/// 新增一个业务域只需在此添加一个分组变体；该域的子命令和执行逻辑均留在自己的模块。
#[derive(Debug, Subcommand)]
pub enum Command {
    Profile {
        #[command(subcommand)]
        command: profile::ProfileCommand,
    },
    Session {
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
