//! 从固定 Profile 路径构造 RPC 运行时依赖。

use std::sync::Arc;

use anyhow::{Context, Result};
use sagent_config::{SagentPaths, read_public_config, resolve_openai_provider};
use sagent_provider::ModelProvider;
use sagent_runtime::SessionSupervisor;
use sagent_store::Store;

use crate::service::RuntimeService;

/// 已绑定一个 Profile 的运行时装配结果。
///
/// Bootstrap 在启动时固定路径与 Provider；RPC 请求不能传入 home、profile、model、
/// endpoint 或 API key，因而不会在同一个 daemon 内跨越 Profile 或凭据边界。
pub struct RuntimeBootstrap {
    service: RuntimeService,
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

        let read_store = Store::open_readonly(&state_db)
            .with_context(|| format!("打开 RPC 只读数据库失败：{}", state_db.display()))?;
        let service = RuntimeService::new(
            sagent_protocol::SessionService::new(read_store),
            state_db,
            model,
            Arc::new(supervisor),
            provider_ready,
            sagent_protocol::ConfigReadResult {
                profile: public_config.profile,
                provider: public_config.provider,
                model: public_config.model,
                provider_names: public_config.provider_names,
                unknown_fields: public_config.unknown_fields,
            },
        );
        Ok(Self { service })
    }

    /// 交出唯一的 RPC 服务实例给异步 transport。
    pub fn into_service(self) -> RuntimeService {
        self.service
    }
}
