//! SessionSupervisor 与 SessionActor 的依赖装配。
//!
//! 本模块只负责把 bootstrap 已解析的存储、Provider、工具和策略参数组成不可变
//! 快照；不管理 Session 生命周期，也不执行命令。`SessionSupervisor` 消费该快照后，
//! 每个新 actor 都取得独占存储端口与相同的启动期能力配置。

use std::sync::Arc;
use std::time::Duration;

use sagent_provider::ModelProvider;
use sagent_store::{
    SessionQueryStorage, Storage, StorageDependencies as DomainStorageDependencies, StorageFactory,
    StorageManager,
};
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

/// 单个新 Actor 使用的业务存储提供器。
///
/// 每次调用必须返回独立依赖，保证 actor 是写入端口的唯一拥有者。失败原因会映射为
/// `RuntimeError::Persistence`，不会泄漏具体数据库实现。
type ActorStorageProvider = Arc<dyn Fn() -> Result<Storage, String> + Send + Sync>;
/// 为 RPC 查询服务创建独立只读查询端口的闭包。
type QueryStorageProvider =
    Arc<dyn Fn() -> Result<Box<dyn SessionQueryStorage>, String> + Send + Sync>;

/// 负责为每个 actor 创建独占领域存储端口的运行时依赖。
pub(crate) struct RuntimeStorageDependencies {
    pub(crate) actor_storage_provider: ActorStorageProvider,
    query_storage_provider: QueryStorageProvider,
}

impl RuntimeStorageDependencies {
    /// 从兼容期的具体存储工厂创建运行时依赖。
    fn from_factory<F, D>(factory: F) -> Self
    where
        F: Fn() -> Result<D, String> + Send + Sync + 'static,
        D: Into<DomainStorageDependencies> + 'static,
    {
        let actor_storage_provider: ActorStorageProvider =
            Arc::new(move || factory().map(Into::into).map(Storage::from_dependencies));
        let query_storage_provider =
            query_provider_from_actor_provider(Arc::clone(&actor_storage_provider));
        Self {
            actor_storage_provider,
            query_storage_provider,
        }
    }

    /// 从抽象 StorageFactory 创建一个只属于当前 actor 的存储依赖。
    fn from_storage_factory(factory: Arc<dyn StorageFactory>) -> Self {
        Self::from_factory(move || factory.create().map_err(|error| error.to_string()))
    }

    /// 从 Profile 级 Manager 创建 Actor 独占存储和连接级只读查询端口。
    ///
    /// Actor 与查询使用 Manager 的不同申请入口：前者取得完整 `Storage`，后者只能
    /// 取得 `ReadStorage` 中的查询端口。这样迁移期仍可让 Actor 构造边界消费旧依赖，
    /// 但不会为了查询而打开可写端口。
    fn from_storage_manager(manager: Arc<dyn StorageManager>) -> Self {
        let actor_manager = Arc::clone(&manager);
        let actor_storage_provider: ActorStorageProvider = Arc::new(move || {
            actor_manager
                .open_actor_storage()
                .map_err(|error| error.to_string())
        });
        let query_storage_provider: QueryStorageProvider = Arc::new(move || {
            let storage = manager
                .open_read_storage()
                .map_err(|error| error.to_string())?;
            let (query, _search) = storage.into_parts();
            Ok(query)
        });
        Self {
            actor_storage_provider,
            query_storage_provider,
        }
    }

    /// 创建一个只属于当前 actor 的存储聚合，并映射为稳定的运行时错误。
    fn create(&self) -> Result<Storage, RuntimeError> {
        (self.actor_storage_provider)().map_err(RuntimeError::Persistence)
    }

    /// 为只读 transport 创建一个独占查询端口，并复用 Supervisor 已冻结的存储提供器。
    ///
    /// 查询服务不能借用某个 Actor 的写入端口：Actor 退出、切换会话或关闭 mailbox
    /// 都不应影响已经建立的 RPC 连接。因此这里仍创建连接级端口，但创建入口由同一
    /// 份运行时依赖统一管理，避免 Bootstrap 再保存一条独立的数据库装配路径。Factory
    /// 兼容路径和 Manager 路径都遵守这一边界。
    pub(crate) fn create_query_storage(
        &self,
    ) -> Result<Box<dyn SessionQueryStorage>, RuntimeError> {
        (self.query_storage_provider)().map_err(RuntimeError::Persistence)
    }
}

/// 为兼容期 Factory 派生只读查询闭包；Manager 路径使用独立的 `open_read_storage`。
fn query_provider_from_actor_provider(
    actor_storage_provider: ActorStorageProvider,
) -> QueryStorageProvider {
    Arc::new(move || {
        let storage = actor_storage_provider()?;
        let (_write, query, _search) = storage.into_parts();
        Ok(query)
    })
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

/// SessionSupervisor 启动 SessionActor 所需的已解析依赖快照。
///
/// 该对象只在 bootstrap 或测试装配阶段按值构建；交给 Supervisor 后不可再修改，
/// 从而避免活跃 Session 在 Turn 中途更换 Provider、工具集合或审批策略。
pub struct SessionSupervisorDependencies {
    storage: RuntimeStorageDependencies,
    model: ModelRuntime,
    tools: ToolRuntime,
    policy: RuntimePolicy,
}

impl SessionSupervisorDependencies {
    /// 从每 actor 独占的存储工厂创建默认依赖集合。
    ///
    /// Provider 与工具依赖默认缺失，以支持只读 RPC、空会话和测试；一旦注入，配置
    /// 会在 Supervisor 接管时冻结。
    pub fn new<F, D>(storage_factory: F) -> Self
    where
        F: Fn() -> Result<D, String> + Send + Sync + 'static,
        D: Into<DomainStorageDependencies> + 'static,
    {
        Self {
            storage: RuntimeStorageDependencies::from_factory(storage_factory),
            model: ModelRuntime::Unconfigured,
            tools: ToolRuntime::Disabled,
            policy: RuntimePolicy::default(),
        }
    }

    /// 从抽象 `StorageFactory` 创建运行时依赖集合。
    ///
    /// Factory 会在每个 Actor 创建时生成新的端口集合；Runtime 只保存 Factory 的
    /// 抽象句柄，不保存数据库路径、连接或具体 SQLite 数据库句柄。
    pub fn from_storage_factory(factory: Arc<dyn StorageFactory>) -> Self {
        Self {
            storage: RuntimeStorageDependencies::from_storage_factory(factory),
            model: ModelRuntime::Unconfigured,
            tools: ToolRuntime::Disabled,
            policy: RuntimePolicy::default(),
        }
    }

    /// 从 Profile 级 `StorageManager` 创建运行时依赖集合。
    ///
    /// Manager 由 bootstrap 持有并可被多个 Actor 共享；每个 Actor 和 RPC 查询仍通过
    /// 独立申请入口取得自己的存储对象。该构造函数是 Runtime 从 Factory 迁移到 Manager
    /// 的正式边界，旧的 `from_storage_factory` 仅为兼容期保留。
    pub fn from_storage_manager(manager: Arc<dyn StorageManager>) -> Self {
        Self {
            storage: RuntimeStorageDependencies::from_storage_manager(manager),
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
/// 此类型不公开，防止 transport 或业务层绕过 `SessionSupervisorDependencies` 在运行中拼接
/// 不完整依赖；它只处理 actor 构造，不拥有 actor 的启动、停止或映射关系。
pub(crate) struct SessionActorFactory {
    storage: RuntimeStorageDependencies,
    model: ModelRuntime,
    tools: ToolRuntime,
    policy: RuntimePolicy,
}

impl SessionActorFactory {
    /// 为 RPC 查询服务提供连接级只读端口；具体后端仍隐藏在冻结的存储提供器之后。
    pub(crate) fn create_query_storage(
        &self,
    ) -> Result<Box<dyn SessionQueryStorage>, RuntimeError> {
        self.storage.create_query_storage()
    }

    /// 使用独占领域存储端口与冻结的能力快照创建一个 actor。
    pub(crate) fn create(
        &self,
        session_id: SessionId,
        command_rx: mpsc::Receiver<ActorInput>,
        command_tx: mpsc::Sender<ActorInput>,
        event_tx: broadcast::Sender<RuntimeEvent>,
    ) -> Result<SessionActor, RuntimeError> {
        let storage = self.storage.create()?;
        let actor = SessionActor::new(session_id, storage, command_rx, command_tx, event_tx);
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
            ToolRuntime::Enabled { dispatcher, worker } => {
                actor.with_tools(dispatcher.clone(), worker.as_ref().clone())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc};

    use sagent_store::{SqliteStorageManager, StorageManager};

    use super::RuntimeStorageDependencies;

    /// Manager 路径必须能分别提供 Actor 完整存储和查询只读端口，且不依赖旧 Factory。
    #[test]
    fn manager_dependencies_open_actor_and_query_storage() {
        let database_path =
            std::env::temp_dir().join(format!("sagent-runtime-manager-{}.db", std::process::id()));
        let _ = fs::remove_file(&database_path);

        // Arrange：先完成 schema 初始化，模拟 Profile bootstrap 已冻结 Manager 的场景。
        let manager = Arc::new(
            SqliteStorageManager::new(&database_path).expect("绝对路径应能创建 SQLite manager"),
        );
        manager
            .initialize()
            .expect("Manager 应能初始化 SQLite schema");
        let dependencies = RuntimeStorageDependencies::from_storage_manager(manager);

        // Act：两个申请入口分别创建完整 Actor 存储和只读查询端口。
        let actor_storage = dependencies.create().expect("Manager 应能提供 Actor 存储");
        let query_storage = dependencies
            .create_query_storage()
            .expect("Manager 应能提供查询存储");

        // Assert：两类对象都能独立取得；具体端口实现仍被 Runtime 边界隐藏。
        drop(actor_storage);
        drop(query_storage);
        let _ = fs::remove_file(database_path);
    }
}
