//! Profile CLI 命令模块的 facade。
//!
//! 子模块分别负责命令协议、命令处理和创建副作用；本文件只声明模块并导出给
//! `commands` 父模块使用的稳定入口，避免 Profile 领域逻辑重新集中到单一文件。
//!
//! 作者：SongZQ

mod command;
mod handler;
mod service;

pub(super) use command::ProfileCommand;
pub(super) use handler::ProfileHandler;
pub(super) use service::ProfileService;
