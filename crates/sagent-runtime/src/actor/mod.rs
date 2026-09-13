//! SessionActor 的模块 facade。
//!
//! 具体职责按主题分布在同级模块中；本文件只声明模块，并重导出 Runtime/Supervisor
//! 需要的 Actor 入口，避免 facade 重新成为集中业务逻辑的“大文件”。

mod context;
mod generation;
mod model_runtime;
mod prompt;
mod session;
mod tool_runtime;
mod tools;
mod turn;
mod worker;

#[cfg(test)]
mod tests;

use context::{ActorChannels, ActorLifecycle, ActorPolicy, ActorSessionContext};
use model_runtime::ActorModelRuntime;
#[cfg(test)]
pub(crate) use model_runtime::WorkerFactory;
use tool_runtime::ActorToolRuntime;

pub(crate) use session::SessionActor;
#[cfg(test)]
pub(crate) use session::utc_now;
