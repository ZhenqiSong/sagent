//! CLI 顶层命令协议。
//!
//! 本模块只定义 Clap 解析后的命令数据，不执行命令、不读取配置，也不创建基础设施。
//!
//! 作者：SongZQ

use clap::Subcommand;

use super::{profile, session};

/// 顶层命令分组。
///
/// 新增一个业务域只需在此添加一个分组变体；该域的子命令和执行逻辑均留在自己的模块。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Profile 子命令分组。
    Profile {
        /// Profile 子命令。
        #[command(subcommand)]
        command: profile::ProfileCommand,
    },
    /// Session 子命令分组。
    Session {
        /// Session 子命令。
        #[command(subcommand)]
        command: session::SessionCommand,
    },
}
