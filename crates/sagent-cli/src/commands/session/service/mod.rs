//! Session 领域服务 facade。
//!
//! 创建、查询和生命周期写操作各自位于按职责命名的子模块；本文件统一声明给
//! `SessionHandler` 使用的服务入口。服务持有 `CommandContext`，不重复解析 Profile
//! 或数据库配置。
//!
//! 作者：SongZQ

use std::time::SystemTime;

use anyhow::Result;

use crate::commands::{CommandContext, storage::CliStorageContext};

mod creation;
mod lifecycle;
mod query;

/// Session 领域服务。
///
/// 服务对象拥有一次 CLI 调用的 `CommandContext`，因此所有会话操作都通过同一个惰性
/// 存储装配入口执行；handler 只负责把命令参数转换为服务调用并格式化结果。
pub(crate) struct SessionService {
    /// 当前 CLI 调用的上下文，包含 Profile 选择、输出格式和共享存储装配。
    context: CommandContext,
}

impl SessionService {
    /// 创建绑定指定命令上下文的 Session 服务。
    pub(crate) fn new(context: CommandContext) -> Self {
        Self { context }
    }

    /// 返回服务持有的命令上下文，供 handler 使用统一输出格式和错误边界。
    pub(crate) fn context(&self) -> &CommandContext {
        &self.context
    }

    /// 为服务子模块取得当前命令共享的存储上下文。
    fn storage(&self) -> Result<&CliStorageContext> {
        self.context.storage()
    }

    /// 生成生命周期操作共用的当前 UTC 毫秒时间戳。
    pub(crate) fn now_rfc3339(&self) -> Result<String> {
        Self::rfc3339_now(SystemTime::now())
    }
}
