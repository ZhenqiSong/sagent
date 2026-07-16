//! sagent-cli — CLI 入口库。
//!
//! 提供子命令模块和辅助函数。

pub mod cli;
pub mod commands;
pub mod config;
pub mod managed_scope;
pub mod cli_core;

pub use cli::SAgentCLI;