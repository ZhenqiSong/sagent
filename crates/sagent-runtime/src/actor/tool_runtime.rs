//! SessionActor 的工具执行模式。
//!
//! 工具 schema 规划和实际执行必须使用同一份已验证依赖，避免只配置 dispatcher 或
//! worker 的半成品状态进入运行时。

use std::sync::Arc;

use crate::{tool_dispatch::ToolDispatcher, tool_worker::ToolWorker};

/// 当前 Actor 的工具执行模式；工具定义校验与实际执行器必须成对存在。
pub(crate) enum ActorToolRuntime {
    /// 当前运行时未启用工具能力，schema 对模型保持为空。
    Disabled,
    /// 已启用工具能力；dispatcher 负责规划/校验，worker 负责执行。
    Enabled {
        dispatcher: ToolDispatcher,
        worker: Arc<ToolWorker>,
    },
}

impl ActorToolRuntime {
    /// 返回工具规划器；未启用工具时返回 `None`，由调用方保持 fail-closed。
    pub(crate) fn dispatcher(&self) -> Option<&ToolDispatcher> {
        match self {
            Self::Disabled => None,
            Self::Enabled { dispatcher, .. } => Some(dispatcher),
        }
    }

    /// 克隆受监管的工具执行器句柄，避免把 Actor 的可变借用带入异步任务。
    pub(crate) fn worker(&self) -> Option<Arc<ToolWorker>> {
        match self {
            Self::Disabled => None,
            Self::Enabled { worker, .. } => Some(worker.clone()),
        }
    }
}
