//! Sagent 的路径、Profile 和 Provider 配置解析。
//!
//! 配置层只负责读取和校验文件，并将结果交给 CLI/RPC/Runtime；它不创建会话或写入
//! SQLite，因此可以在不同入口复用同一套 Profile 隔离规则。

mod config_reader;
mod credentials;
mod profile_config;
mod provider_config;
mod provider_resolver;
mod public_config;
mod workspace;

#[cfg(test)]
mod test_support;

pub mod paths;
pub mod profile;
pub mod storage;

pub use config_reader::load_profile_config;
pub use paths::{SagentPaths, resolve_active_paths, resolve_paths};
pub use profile::{Profile, ProfileInfo, ProfileName, normalize_profile_name};
pub use profile_config::ProfileConfig;
pub use provider_config::{
    ModelDetail, ModelSetting, ProviderDescriptor, UserProviderConfig, WorkspaceDescriptor,
};
pub use provider_resolver::{
    ResolvedProvider, ResolvedProviderConfig, resolve_openai_provider_from_config,
    resolve_provider_config_from_config,
};
pub use public_config::{PublicConfig, read_public_config_from_config};
pub use storage::{DEFAULT_SQLITE_DATABASE_FILE, StorageDescriptor, StorageKind};
pub use workspace::resolve_workspace_from_config;
