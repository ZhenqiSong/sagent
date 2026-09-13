//! 从固定 Profile 路径构造 RPC 运行时依赖。

use std::sync::Arc;

use anyhow::{Context, Result};
use sagent_config::{
    SagentPaths, load_profile_config, read_public_config_from_config,
    resolve_openai_provider_from_config, resolve_workspace_from_config,
};
use sagent_provider::ModelProvider;
use sagent_runtime::{RuntimeDependencies, SessionSupervisor, ToolDispatcher, ToolWorker};
use sagent_store::StorageFactory;
use sagent_tools::{ReadFileLimits, TerminalLimits, WorkspaceRoot, builtin_registry};

use crate::{service::RuntimeService, storage_factory::create_storage_factory};

/// 已绑定一个 Profile 的运行时装配结果。
///
/// Bootstrap 在启动时固定路径与 Provider；RPC 请求不能传入 home、profile、model、
/// endpoint 或 API key，因而不会在同一个 daemon 内跨越 Profile 或凭据边界。
pub struct RuntimeBootstrap {
    storage_factory: Arc<dyn StorageFactory>,
    model: String,
    supervisor: Arc<SessionSupervisor>,
    provider_ready: bool,
    public_config: sagent_protocol::ConfigReadResult,
}

impl RuntimeBootstrap {
    /// 创建存储 Factory、Provider 和每 Actor 独占的领域端口。
    pub fn from_paths(paths: SagentPaths) -> Result<Self> {
        // Profile 配置在 bootstrap 开始时只读取一次；后续 Provider、workspace、公开摘要
        // 和 storage 校验都复用同一快照，避免同一 Runtime 看到不一致的文件内容。
        let profile_config = load_profile_config(&paths).context("读取 Profile 配置失败")?;
        let storage_descriptor = profile_config.get_storage_descriptor();

        // 后端选择只发生在独立的 selector 中；Bootstrap 不根据 StorageKind 分支，
        // 也不持有数据库路径、连接或具体 SQLite 数据库句柄。Factory 的首次 create 负责初始化
        // migration 和连接检查，之后每个 Actor/请求都获取独立的端口集合。
        let storage_factory: Arc<dyn StorageFactory> =
            create_storage_factory(&paths, storage_descriptor)
                .context("创建 Profile 存储 Factory 失败")?;
        storage_factory.create().context("初始化 RPC 存储失败")?;

        // 公开配置在 bootstrap 时冻结；读取失败不能降级为“空配置”，否则客户端会把
        // 损坏 YAML 误认为未配置。
        let public_config = read_public_config_from_config(&paths, &profile_config)
            .context("读取公开 Profile 配置失败")?;
        let dependencies = RuntimeDependencies::from_storage_factory(Arc::clone(&storage_factory));

        // 工具边界必须在 Profile bootstrap 时固定，不能接受来自 prompt.submit 的路径或
        // registry 覆盖。workspace 不可用时只关闭工具而不影响只读 RPC/空会话，让损坏的
        // 工具配置不会阻塞用户恢复已有 transcript；可用时每个 Actor 共享无状态 worker
        // 配置，但实际数据库写入仍由 Actor 独占连接完成。
        let dependencies = match resolve_workspace_from_config(&paths, &profile_config)
            .and_then(|root| WorkspaceRoot::new(root).map_err(|error| anyhow::anyhow!(error)))
        {
            Ok(workspace) => match builtin_registry() {
                Ok(registry) => {
                    let dispatcher = ToolDispatcher::new(registry);
                    let worker = ToolWorker::new(
                        workspace,
                        ReadFileLimits::default(),
                        TerminalLimits::default(),
                    )
                    .with_session_search_factory(Arc::clone(&storage_factory));
                    dependencies.with_tools(dispatcher, worker)
                }
                Err(_) => dependencies,
            },
            Err(_) => dependencies,
        };

        // Provider 缺失不破坏第三阶段的只读 RPC 或空会话创建；后续 prompt.submit 会
        // 使用 provider_ready 返回稳定 runtime_unavailable，而不会泄露 resolver 细节。
        let resolved_provider =
            resolve_openai_provider_from_config(&paths, &profile_config, None, None);
        let (dependencies, model, provider_ready) = match resolved_provider {
            Ok(resolved) => {
                let model = resolved.model;
                let provider: Arc<dyn ModelProvider> = Arc::new(resolved.client);
                (
                    dependencies.with_provider(provider, model.clone(), "rpc-bootstrap-v1"),
                    model,
                    true,
                )
            }
            Err(_) => (dependencies, "unconfigured".to_owned(), false),
        };

        Ok(Self {
            storage_factory,
            model,
            supervisor: Arc::new(SessionSupervisor::new(dependencies)),
            provider_ready,
            public_config: sagent_protocol::ConfigReadResult {
                profile: public_config.profile,
                provider: public_config.provider,
                model: public_config.model,
                provider_names: public_config.provider_names,
                unknown_fields: public_config.unknown_fields,
            },
        })
    }

    /// 为一条 transport 连接装配独占的只读查询端口。
    ///
    /// 查询依赖由 Supervisor 持有的冻结运行时工厂创建，Bootstrap 不再直接打开另一
    /// 个存储依赖。每条连接仍取得自己的查询端口，Supervisor 则跨连接共享，以便重连
    /// 客户端继续控制既有 Actor；连接之间不共享具体数据库连接。
    pub fn open_service(&self) -> Result<RuntimeService> {
        let query_storage = self
            .supervisor
            .create_query_storage()
            .context("打开 RPC 查询存储失败")?;
        Ok(RuntimeService::new(
            sagent_protocol::SessionService::new_boxed(query_storage),
            Arc::clone(&self.storage_factory),
            self.model.clone(),
            Arc::clone(&self.supervisor),
            self.provider_ready,
            self.public_config.clone(),
        ))
    }
}
