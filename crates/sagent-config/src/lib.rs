//! Sagent 的路径、Profile 和 Provider 配置解析。
//!
//! 配置层只负责读取和校验文件，并将结果交给 CLI/RPC/Runtime；它不创建会话或写入
//! SQLite，因此可以在不同入口复用同一套 Profile 隔离规则。

pub mod paths;
pub mod profile;
pub mod provider;

pub use paths::{SagentPaths, resolve_active_paths, resolve_paths};
pub use profile::{
    ProfileName, active_profile_path, list_profile_names, normalize_profile_name,
    read_active_profile, set_active_profile,
};
pub use provider::{
    ProviderConfig, PublicConfig, ResolvedProvider, ResolvedProviderConfig, read_public_config,
    resolve_openai_provider, resolve_provider_config,
};
