//! 从固定 Profile 路径构造 RPC 运行时依赖。

use std::sync::Arc;

use anyhow::{Context, Result};
use sagent_config::{SagentPaths, read_public_config, resolve_openai_provider, resolve_workspace};
use sagent_provider::ModelProvider;
use sagent_runtime::{SessionSupervisor, ToolDispatcher, ToolWorker};
use sagent_store::Store;
use sagent_tools::{ReadFileLimits, TerminalLimits, WorkspaceRoot, builtin_registry};

use crate::service::RuntimeService;

/// 已绑定一个 Profile 的运行时装配结果。
///
/// Bootstrap 在启动时固定路径与 Provider；RPC 请求不能传入 home、profile、model、
/// endpoint 或 API key，因而不会在同一个 daemon 内跨越 Profile 或凭据边界。
pub struct RuntimeBootstrap {
    state_db: std::path::PathBuf,
    model: String,
    supervisor: Arc<SessionSupervisor>,
    provider_ready: bool,
    public_config: sagent_protocol::ConfigReadResult,
}

impl RuntimeBootstrap {
    /// 创建数据库、Provider 和每 Actor 独占的 Store factory。
    pub fn from_paths(paths: SagentPaths) -> Result<Self> {
        // 首次启动允许创建 Sagent 自有 state.db 并执行加性 migration；随后读服务和
        // 每个 Actor 都各自打开连接，绝不跨 Actor 共享 rusqlite Connection。
        let initialization_store = Store::open_readwrite(&paths.state_db)
            .with_context(|| format!("初始化 RPC 数据库失败：{}", paths.state_db.display()))?;
        initialization_store
            .verify_connection()
            .context("RPC 数据库连接检查失败")?;
        drop(initialization_store);

        let state_db = paths.state_db.clone();
        // 公开配置在 bootstrap 时冻结；读取失败不能降级为“空配置”，否则客户端会把
        // 损坏 YAML 误认为未配置。
        let public_config = read_public_config(&paths).context("读取公开 Profile 配置失败")?;
        let store_factory_path = state_db.clone();
        let base_supervisor = SessionSupervisor::new(move || {
            Store::open_readwrite(&store_factory_path)
                .map_err(|error| format!("无法打开 actor 数据库：{error}"))
        });

        // 工具边界必须在 Profile bootstrap 时固定，不能接受来自 prompt.submit 的路径或
        // registry 覆盖。workspace 不可用时只关闭工具而不影响只读 RPC/空会话，让损坏的
        // 工具配置不会阻塞用户恢复已有 transcript；可用时每个 Actor 共享无状态 worker
        // 配置，但实际 Store 写入仍由 Actor 独占连接完成。
        let base_supervisor = match resolve_workspace(&paths)
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
                    .with_session_search(state_db.clone());
                    base_supervisor
                        .with_tool_dispatcher(dispatcher)
                        .with_tool_worker(worker)
                }
                Err(_) => base_supervisor,
            },
            Err(_) => base_supervisor,
        };

        // Provider 缺失不破坏第三阶段的只读 RPC 或空会话创建；后续 prompt.submit 会
        // 使用 provider_ready 返回稳定 runtime_unavailable，而不会泄露 resolver 细节。
        let resolved_provider = resolve_openai_provider(&paths, None, None);
        let (supervisor, model, provider_ready) = match resolved_provider {
            Ok(resolved) => {
                let model = resolved.model;
                let provider: Arc<dyn ModelProvider> = Arc::new(resolved.client);
                (
                    base_supervisor.with_provider(provider, model.clone(), "rpc-bootstrap-v1"),
                    model,
                    true,
                )
            }
            Err(_) => (base_supervisor, "unconfigured".to_owned(), false),
        };

        Ok(Self {
            state_db,
            model,
            supervisor: Arc::new(supervisor),
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

    /// 为一条 transport 连接打开独占的只读 Store 适配层。
    ///
    /// `rusqlite::Connection` 不应跨 WebSocket 连接共享；Supervisor 则必须共享，才能让
    /// 同一 Profile 的重连客户端继续控制既有 Actor。因此每条连接新建读服务、复用同一
    /// Actor 注册表和启动期配置快照。
    pub fn open_service(&self) -> Result<RuntimeService> {
        let read_store = Store::open_readonly(&self.state_db)
            .with_context(|| format!("打开 RPC 只读数据库失败：{}", self.state_db.display()))?;
        Ok(RuntimeService::new(
            sagent_protocol::SessionService::new(read_store),
            self.state_db.clone(),
            self.model.clone(),
            Arc::clone(&self.supervisor),
            self.provider_ready,
            self.public_config.clone(),
        ))
    }
}
