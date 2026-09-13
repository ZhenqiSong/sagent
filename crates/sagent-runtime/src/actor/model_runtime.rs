//! SessionActor 的模型执行模式。
//!
//! 模型身份与执行入口必须成组保存，避免 Provider、测试 worker 和 generation 元数据
//! 通过多个可空字段组合出互相矛盾的状态。

use std::sync::Arc;

use sagent_provider::ModelProvider;
#[cfg(test)]
use tokio::sync::mpsc;
#[cfg(test)]
use tokio::task::JoinHandle;
#[cfg(test)]
use tokio_util::sync::CancellationToken;

#[cfg(test)]
use crate::input::ActorInput;

/// 测试替身或后续 Provider 用来启动 worker 的函数。
#[cfg(test)]
pub(crate) type WorkerFactory = Arc<
    dyn Fn(mpsc::Sender<ActorInput>, sagent_types::TurnId, CancellationToken) -> JoinHandle<()>
        + Send
        + Sync,
>;

/// 当前 Actor 使用的模型执行模式；模型身份与实际执行入口必须成组存在。
///
/// `Unconfigured` 用于只提供会话/持久化能力的运行时，`Provider` 是生产模型调用路径，
/// `Test` 仅用于注入确定性的测试 worker。用枚举表达模式可以避免 provider、测试工厂
/// 和模型元数据形成互相矛盾的 `Option` 组合。
pub(crate) enum ActorModelRuntime {
    /// 未配置模型执行器，但仍保留 generation 所需的默认模型身份。
    Unconfigured {
        model: String,
        profile_revision: String,
    },
    /// 使用真实 Provider 处理模型回合。
    Provider {
        provider: Arc<dyn ModelProvider>,
        model: String,
        profile_revision: String,
    },
    /// 测试专用的受控 worker。
    #[cfg(test)]
    Test {
        worker_factory: WorkerFactory,
        model: String,
        profile_revision: String,
    },
}

impl ActorModelRuntime {
    /// 返回当前 generation 需要持久化的模型标识。
    pub(crate) fn model(&self) -> &str {
        match self {
            Self::Unconfigured { model, .. } | Self::Provider { model, .. } => model,
            #[cfg(test)]
            Self::Test { model, .. } => model,
        }
    }

    /// 返回当前 generation 需要持久化的 profile 版本。
    pub(crate) fn profile_revision(&self) -> &str {
        match self {
            Self::Unconfigured {
                profile_revision, ..
            }
            | Self::Provider {
                profile_revision, ..
            } => profile_revision,
            #[cfg(test)]
            Self::Test {
                profile_revision, ..
            } => profile_revision,
        }
    }
}
