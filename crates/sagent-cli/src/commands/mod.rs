//! 按业务边界拆分的 CLI 命令实现。
//!
//! 作者：SongZQ

use std::path::Path;

use anyhow::Result;
use clap::Subcommand;

use crate::output::OutputFormat;

pub mod profile;
pub mod session;

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
    pub fn execute(
        self,
        home: Option<&Path>,
        profile: Option<&str>,
        format: OutputFormat,
    ) -> Result<()> {
        match self {
            Self::Profile { command } => command.execute(home, format),
            Self::Session { command } => command.execute(home, profile, format),
        }
    }
}
