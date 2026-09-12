//! SessionActor 的运行时依赖装配。
//!
//! 本模块只负责把 bootstrap 已解析的 Store、Provider、工具和策略参数组成不可变
//! 快照；不管理 Session 生命周期，也不执行命令。`SessionSupervisor` 消费该快照后，
//! 每个新 actor 都取得独占 Store 与相同的启动期能力配置。

use std::sync::Arc;
use std::time::Duration;

use sagent_provider::ModelProvider;
use sagent_store::Store;
use sagent_types::SessionId;
use tokio::sync::{broadcast, mpsc};

use crate::RuntimeError;
use crate::actor::SessionActor;
#[cfg(test)]
use crate::actor::{WorkerFactory, utc_now};
use crate::event::RuntimeEvent;
use crate::input::ActorInput;
use crate::tool_dispatch::ToolDispatcher;
use crate::tool_worker::ToolWorker;

/// 单个新 actor 使用的 Store 打开函数。
///
/// 每次调用必须返回独立连接，保证 actor 是 Store 的唯一写入者。失败原因会映射为
/// `RuntimeError::Persistence`，不会泄漏具体数据库实现。
type StoreFactory = Arc<dyn Fn() -> Result<Store, String> + Send + Sync>;

/// 负责为每个 actor 创建独占 Store 的持久化依赖。
pub(crate) struct StorageDependencies {
    pub(crate) store_factory: StoreFactory,
}

impl StorageDependencies {
    /// 从调用方提供的 Store 工厂创建持久化依赖。
    fn new<F>(store_factory: F) -> Self
    where
        F: Fn() -> Result<Store, String> + Send + Sync + 'static,
    {
        Self {
            store_factory: Arc::new(store_factory),
        }
    }

    /// 打开一个只属于当前 actor 的 Store，并映射为稳定的运行时错误。
    fn open_store(&self) -> Result<Store, RuntimeError> {
        (self.store_factory)().map_err(RuntimeError::Persistence)
    }
}

/// 已解析的模型调用依赖，保证 Provider 与其模型元数据成组传递。
pub(crate) struct ModelDependencies {
    pub(crate) provider: Arc<dyn ModelProvider>,
    pub(crate) model: String,
    pub(crate) profile_revision: String,
}

/// 模型调用的已验证运行模式。
pub(crate) enum ModelRuntime {
    /// Provider 尚未配置；只允许启动空会话，提交时由 Actor 返回稳定错误。
    Unconfigured,
    /// 使用真实 Provider 处理模型回合。
    Provider(ModelDependencies),
    /// 测试专用的受控 worker，不与真实 Provider 同时存在。
    #[cfg(test)]
    Test(WorkerFactory),
}

/// 工具调用的已验证运行模式。
pub(crate) enum ToolRuntime {
    /// 当前 Profile 未启用工具能力。
    Disabled,
    /// dispatcher 与 worker 必须成对出现，保证 schema 验证和执行边界一致。
    Enabled {
        /// 负责工具定义、schema 和风险校验。
        dispatcher: ToolDispatcher,
        /// 负责执行已通过 dispatcher 校验的调用。
        worker: Box<ToolWorker>,
    },
}

/// SessionActor 使用的运行时策略值。
pub(crate) struct RuntimePolicy {
    pub(crate) approval_timeout: Duration,
    pub(crate) max_tool_rounds: u32,
}

impl Default for RuntimePolicy {
    fn default() -> Self {
        Self {
            approval_timeout: Duration::from_secs(300),
            max_tool_rounds: 8,
        }
    }
}

/// 运行时启动一个 SessionActor 所需的已解析依赖。
///
/// 该对象只在 bootstrap 或测试装配阶段按值构建；交给 Supervisor 后不可再修改，
/// 从而避免活跃 Session 在 Turn 中途更换 Provider、工具集合或审批策略。
pub struct RuntimeDependencies {
    storage: StorageDependencies,
    model: ModelRuntime,
    tools: ToolRuntime,
    policy: RuntimePolicy,
}

impl RuntimeDependencies {
    /// 从每 actor 独占的 Store 工厂创建默认依赖集合。
    ///
    /// Provider 与工具依赖默认缺失，以支持只读 RPC、空会话和测试；一旦注入，配置
    /// 会在 Supervisor 接管时冻结。
    pub fn new<F>(store_factory: F) -> Self
    where
        F: Fn() -> Result<Store, String> + Send + Sync + 'static,
    {
        Self {
            storage: StorageDependencies::new(store_factory),
            model: ModelRuntime::Unconfigured,
            tools: ToolRuntime::Disabled,
            policy: RuntimePolicy::default(),
        }
    }

    /// 注入当前 Profile 已解析的模型 Provider 与其版本标识。
    pub fn with_provider(
        mut self,
        provider: Arc<dyn ModelProvider>,
        model: impl Into<String>,
        profile_revision: impl Into<String>,
    ) -> Self {
        self.model = ModelRuntime::Provider(ModelDependencies {
            provider,
            model: model.into(),
            profile_revision: profile_revision.into(),
        });
        self
    }

    /// 覆盖单个 pending approval 的最大等待时间。
    pub fn with_approval_timeout(mut self, timeout: Duration) -> Self {
        self.policy.approval_timeout = timeout;
        self
    }

    /// 一次性启用工具定义与执行能力，避免只配置其中一侧。
    pub fn with_tools(mut self, dispatcher: ToolDispatcher, worker: ToolWorker) -> Self {
        self.tools = ToolRuntime::Enabled {
            dispatcher,
            worker: Box::new(worker),
        };
        self
    }

    /// 覆盖单个 Turn 可完成的工具调用批次上限，零值收敛为一批。
    pub fn with_max_tool_rounds(mut self, limit: u32) -> Self {
        self.policy.max_tool_rounds = limit.max(1);
        self
    }

    /// 为单元测试注入受控 worker，生产路径应使用 `with_provider`。
    #[cfg(test)]
    pub(crate) fn with_worker_factory(mut self, worker_factory: WorkerFactory) -> Self {
        self.model = ModelRuntime::Test(worker_factory);
        self
    }

    /// 消费装配结果，生成 Supervisor 专用的 actor 工厂。
    pub(crate) fn into_actor_factory(self) -> SessionActorFactory {
        SessionActorFactory {
            storage: self.storage,
            model: self.model,
            tools: self.tools,
            policy: self.policy,
        }
    }
}

/// 根据冻结依赖创建单个 SessionActor 的内部工厂。
///
/// 此类型不公开，防止 transport 或业务层绕过 `RuntimeDependencies` 在运行中拼接
/// 不完整依赖；它只处理 actor 构造，不拥有 actor 的启动、停止或映射关系。
pub(crate) struct SessionActorFactory {
    storage: StorageDependencies,
    model: ModelRuntime,
    tools: ToolRuntime,
    policy: RuntimePolicy,
}

impl SessionActorFactory {
    /// 使用独占 Store 与冻结的能力快照创建一个 actor。
    pub(crate) fn create(
        &self,
        session_id: SessionId,
        command_rx: mpsc::Receiver<ActorInput>,
        command_tx: mpsc::Sender<ActorInput>,
        event_tx: broadcast::Sender<RuntimeEvent>,
    ) -> Result<SessionActor, RuntimeError> {
        let store = self.storage.open_store()?;
        let actor = SessionActor::new(session_id, store, command_rx, command_tx, event_tx);
        let actor = actor.with_approval_timeout(self.policy.approval_timeout);
        let actor = actor.with_max_tool_rounds(self.policy.max_tool_rounds);
        let actor = match &self.model {
            ModelRuntime::Unconfigured => actor,
            ModelRuntime::Provider(model) => actor.with_provider(
                model.provider.clone(),
                model.model.clone(),
                model.profile_revision.clone(),
            ),
            #[cfg(test)]
            ModelRuntime::Test(factory) => actor.with_worker_factory(factory.clone(), utc_now),
        };
        Ok(match &self.tools {
            ToolRuntime::Disabled => actor,
            ToolRuntime::Enabled { dispatcher, worker } => actor
                .with_tool_dispatcher(dispatcher.clone())
                .with_tool_worker(worker.as_ref().clone()),
        })
    }
}
