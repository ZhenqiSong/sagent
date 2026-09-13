//! CLI 命令模块的 facade。
//!
//! 具体实现按上下文、命令协议、handler 和业务域拆分；本文件只声明模块并 re-export
//! 对外稳定使用的命令类型与处理入口。
//!
//! 作者：SongZQ

mod command;
mod context;
mod handler;
mod profile;
mod session;

mod storage;

pub use command::Command;
pub use context::CommandContext;
pub(crate) use handler::{CommandHandler, HandlerFactory};
