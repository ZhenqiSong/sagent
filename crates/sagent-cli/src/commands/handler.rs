//! CLI 命令 handler 的统一边界与选择工厂。
//!
//! 本模块只负责定义 handler 执行协议和根据顶层命令选择领域实现；Profile、Session 的
//! 参数规则、存储操作和输出编排分别保留在各自模块。
//!
//! 作者：SongZQ

use anyhow::Result;

use super::{Command, CommandContext, profile, session};

/// CLI 命令处理器的统一执行边界。
///
/// 具体 handler 自己拥有 `CommandContext`，命令值由执行调用显式传入；工厂只负责根据
/// 顶层命令类型选择正确的实现，避免 handler 保存待执行的命令状态。
pub(crate) trait CommandHandler {
    /// 执行调用方传入的已解析命令。
    ///
    /// Handler 自己拥有命令上下文，但不拥有命令值；这样同一处理器可以在需要时执行
    /// 多个同领域命令，同时每次执行的输入边界仍然清晰。可变借用允许 handler 在命令
    /// 成功后同步更新自身持有的领域索引，例如新建 Profile 后写回 map。
    fn execute(&mut self, command: Command) -> Result<()>;
}

/// 根据顶层 CLI 命令创建对应领域处理器。
pub(crate) struct HandlerFactory;

impl HandlerFactory {
    /// 根据命令类型创建拥有命令上下文的领域 handler。
    ///
    /// Profile handler 会在装配阶段解析并校验 home，因此非法路径会在进入业务执行前
    /// 返回；Session handler 当前无需额外装配校验。
    pub(crate) fn create(
        command: &Command,
        context: CommandContext,
    ) -> Result<Box<dyn CommandHandler>> {
        match command {
            Command::Profile { .. } => Ok(Box::new(profile::ProfileHandler::new(context)?)),
            Command::Session { .. } => Ok(Box::new(session::SessionHandler::new(context))),
        }
    }
}
