//! Session CLI 命令模块的 facade。
//!
//! 命令参数、命令处理和会话领域服务分别位于独立子模块；本文件只声明模块并向
//! `commands` 父模块导出顶层命令类型与处理器，避免 Session 逻辑重新聚合到单一文件。
//!
//! 作者：SongZQ

mod command;
mod handler;
mod service;

pub(super) use command::SessionCommand;
pub(super) use handler::SessionHandler;
