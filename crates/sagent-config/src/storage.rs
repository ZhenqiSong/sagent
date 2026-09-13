//! Profile 级存储意图描述。
//!
//! 本模块只负责把配置文件中的存储选择解析为不可变 descriptor；它不打开数据库、
//! 创建连接或读取远程凭据。真正的后端选择和连接创建属于后续 StorageFactory/Bootstrap。

use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::{SagentPaths, profile_config::ProfileConfig};

/// SQLite 未显式配置文件路径时使用的 Profile 内默认文件名。
pub const DEFAULT_SQLITE_DATABASE_FILE: &str = "state.db";

/// 当前配置能够表达的存储后端类别。
#[derive(Debug, Clone, Copy, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    /// Profile 本地 SQLite 文件；未配置 storage 时的默认后端。
    #[default]
    Sqlite,
    /// 由后续 StorageFactory 创建的远程存储连接。
    ///
    /// 当前阶段只解析其连接引用，不代表远程后端已经可用；Bootstrap 在后续工作包
    /// 接入具体实现前必须显式拒绝该类型，不能静默回退到 SQLite。
    Remote,
}

/// 描述 Profile 应使用哪一种持久化后端及其非秘密定位信息。
///
/// `path` 允许相对 Profile 的路径，后续 StorageFactory 负责将其锚定到
/// [`crate::SagentPaths::sagent_home`]；`connection_env` 只保存凭据/连接串的
/// 环境变量名，绝不保存环境变量值。该类型不执行 I/O，因此可安全用于配置校验和公开
/// capability 计算，但不应直接作为数据库连接使用。
#[derive(Debug, Clone, Default, Deserialize, Eq, PartialEq)]
pub struct StorageDescriptor {
    /// 存储后端类别；省略时为本地 SQLite。
    #[serde(default)]
    pub kind: StorageKind,
    /// SQLite 文件路径；相对路径相对于当前 Profile，省略时使用 `state.db`。
    pub path: Option<PathBuf>,
    /// 远程连接信息所在的环境变量名；只允许保存名称，不保存连接串。
    pub connection_env: Option<String>,
    /// 远程后端使用的逻辑 schema；SQLite 实现可忽略该字段。
    pub schema: Option<String>,
    /// 远程后端使用的租户或数据命名空间；SQLite 实现可忽略该字段。
    pub namespace: Option<String>,
    /// 是否以只读策略打开后端；默认允许写入，以保持现有 SQLite 行为。
    #[serde(default)]
    pub read_only: bool,
}

/// 从已经读取的 Profile 配置提取并校验存储意图，不产生额外文件 I/O。
///
/// Bootstrap 如果同时需要 Provider、workspace 和 storage，应先读取一次 `ProfileConfig`，
/// 再把同一快照传入各 resolver；本函数不会产生额外文件 I/O。
pub fn resolve_storage_descriptor_from_config(config: &ProfileConfig) -> Result<StorageDescriptor> {
    config.storage.validate()?;
    Ok(config.storage.clone())
}

/// 根据 Profile 快照解析当前 SQLite 数据库文件。
///
/// 未显式配置 SQLite 文件时使用本模块的默认文件名；相对路径固定锚定当前 Profile，
/// 绝对路径保留用户的明确选择。本函数只做路径计算，不创建目录、文件或数据库连接。
pub fn resolve_sqlite_database_path(
    paths: &SagentPaths,
    config: &ProfileConfig,
) -> Result<PathBuf> {
    let descriptor = &config.storage;
    descriptor.validate()?;
    if descriptor.kind != StorageKind::Sqlite {
        bail!("当前 bootstrap 只支持 sqlite storage")
    }

    Ok(descriptor.path.as_ref().map_or_else(
        || paths.sagent_home.join(DEFAULT_SQLITE_DATABASE_FILE),
        |path| {
            if path.is_absolute() {
                path.clone()
            } else {
                paths.sagent_home.join(path)
            }
        },
    ))
}

/// 检查当前 SQLite bootstrap 是否能完整执行 Profile 的 storage 意图。
///
/// StorageFactory 尚未接入前，Runtime 只能使用 SQLite 的读写模式；遇到远程、
/// schema/namespace 或只读策略时必须失败关闭，避免用户配置被静默忽略。自定义 SQLite
/// 路径由 `resolve_sqlite_database_path` 负责锚定和使用。
pub fn ensure_legacy_bootstrap_supported(config: &ProfileConfig) -> Result<()> {
    let descriptor = &config.storage;
    descriptor.validate()?;
    if descriptor.kind != StorageKind::Sqlite {
        bail!("remote storage 后端尚未实现")
    }
    if descriptor.schema.is_some() || descriptor.namespace.is_some() || descriptor.read_only {
        bail!("当前 SQLite bootstrap 仅支持读写模式，schema/namespace 尚未实现")
    }
    Ok(())
}

impl StorageDescriptor {
    /// 校验 descriptor 的字段组合，不创建任何后端连接。
    pub fn validate(&self) -> Result<()> {
        validate_optional_name("schema", self.schema.as_deref())?;
        validate_optional_name("namespace", self.namespace.as_deref())?;
        match self.kind {
            StorageKind::Sqlite => {
                if self.connection_env.is_some() {
                    bail!("sqlite storage 不能配置 connection_env")
                }
                if self
                    .path
                    .as_ref()
                    .is_some_and(|path| path.as_os_str().is_empty())
                {
                    bail!("sqlite storage 的 path 不能是空路径")
                }
            }
            StorageKind::Remote => {
                if self.path.is_some() {
                    bail!("remote storage 不能配置 sqlite path")
                }
                validate_connection_env(self.connection_env.as_deref())?;
            }
        }
        Ok(())
    }
}

/// 校验可选的逻辑命名字段，避免空白配置在后端实现之间产生不同含义。
fn validate_optional_name(field: &str, value: Option<&str>) -> Result<()> {
    if value.is_some_and(|value| value.trim().is_empty()) {
        bail!("storage.{field} 不能是空白字符串")
    }
    Ok(())
}

/// 校验连接引用确实是环境变量名，阻止把连接串本身或带空格的名称写入配置。
fn validate_connection_env(value: Option<&str>) -> Result<()> {
    let Some(value) = value else {
        bail!("remote storage 必须配置 connection_env")
    };
    if value.trim().is_empty() {
        bail!("storage.connection_env 不能是空白字符串")
    }
    if value != value.trim()
        || value
            .chars()
            .any(|character| character == '=' || character == '\0')
    {
        bail!("storage.connection_env 必须是环境变量名，不能包含首尾空格、= 或 NUL")
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{StorageDescriptor, StorageKind};
    use crate::{ProfileConfig, load_profile_config, resolve_paths};
    use std::{fs, path::PathBuf};

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "sagent-storage-config-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试目录");
        root
    }

    #[test]
    fn missing_storage_uses_sqlite_defaults() {
        let descriptor = StorageDescriptor::default();
        assert_eq!(descriptor.kind, StorageKind::Sqlite);
        assert_eq!(descriptor.path, None);
        assert_eq!(descriptor.connection_env, None);
        assert_eq!(descriptor.schema, None);
        assert_eq!(descriptor.namespace, None);
        assert!(!descriptor.read_only);
        assert!(descriptor.validate().is_ok());
    }

    #[test]
    fn parses_profile_relative_sqlite_descriptor() {
        let descriptor: StorageDescriptor =
            serde_yaml::from_str(
                "kind: sqlite\npath: data/state.db\nschema: app\nnamespace: profile-a\nread_only: true\n",
            )
                .expect("sqlite descriptor 应能解析");
        assert_eq!(descriptor.kind, StorageKind::Sqlite);
        assert_eq!(
            descriptor.path.as_deref(),
            Some(std::path::Path::new("data/state.db"))
        );
        assert_eq!(descriptor.schema.as_deref(), Some("app"));
        assert_eq!(descriptor.namespace.as_deref(), Some("profile-a"));
        assert!(descriptor.read_only);
        assert!(descriptor.validate().is_ok());
    }

    #[test]
    fn remote_descriptor_requires_connection_reference() {
        let descriptor: StorageDescriptor =
            serde_yaml::from_str("kind: remote\nconnection_env: SAGENT_DB_URL\n")
                .expect("remote descriptor 应能解析");
        assert_eq!(descriptor.kind, StorageKind::Remote);
        assert!(descriptor.validate().is_ok());
        let missing: StorageDescriptor =
            serde_yaml::from_str("kind: remote\n").expect("缺少字段仍应先完成反序列化");
        assert!(missing.validate().is_err());

        let with_sqlite_path: StorageDescriptor =
            serde_yaml::from_str("kind: remote\nconnection_env: SAGENT_DB_URL\npath: state.db\n")
                .expect("远程 descriptor 应能完成反序列化");
        assert!(with_sqlite_path.validate().is_err());

        let blank_connection: StorageDescriptor =
            serde_yaml::from_str("kind: remote\nconnection_env: ' SAGENT_DB_URL '\n")
                .expect("带空格的连接引用应能先完成反序列化");
        assert!(blank_connection.validate().is_err());
    }

    #[test]
    fn sqlite_rejects_remote_connection_reference() {
        let descriptor: StorageDescriptor =
            serde_yaml::from_str("kind: sqlite\nconnection_env: SAGENT_DB_URL\n")
                .expect("sqlite descriptor 应能解析");
        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn rejects_blank_logical_names_and_paths() {
        let blank_schema: StorageDescriptor =
            serde_yaml::from_str("schema: '  '\n").expect("空白 schema 应能先完成反序列化");
        assert!(blank_schema.validate().is_err());

        let blank_path: StorageDescriptor =
            serde_yaml::from_str("path: ''\n").expect("空 path 应能先完成反序列化");
        assert!(blank_path.validate().is_err());
    }

    #[test]
    fn extracts_descriptor_from_loaded_config_without_io() {
        let configured = StorageDescriptor {
            kind: StorageKind::Remote,
            path: None,
            connection_env: Some("SAGENT_DB_URL".into()),
            schema: Some("app".into()),
            namespace: Some("profile-a".into()),
            read_only: true,
        };

        let config = ProfileConfig {
            storage: configured.clone(),
            provider: crate::provider_config::ProviderDescriptor {
                provider: None,
                model: None,
                base_url: None,
                api_key_env: None,
                providers: std::collections::BTreeMap::new(),
            },
            workspace: crate::provider_config::WorkspaceDescriptor::default(),
            unknown_fields: Vec::new(),
        };

        let descriptor = super::resolve_storage_descriptor_from_config(&config).expect("应能提取");

        assert_eq!(descriptor, configured);
    }

    #[test]
    fn accepts_custom_sqlite_path_for_legacy_bootstrap() {
        let config = ProfileConfig {
            storage: StorageDescriptor {
                kind: StorageKind::Sqlite,
                path: Some("custom.db".into()),
                connection_env: None,
                schema: None,
                namespace: None,
                read_only: false,
            },
            provider: crate::provider_config::ProviderDescriptor {
                provider: None,
                model: None,
                base_url: None,
                api_key_env: None,
                providers: std::collections::BTreeMap::new(),
            },
            workspace: crate::provider_config::WorkspaceDescriptor::default(),
            unknown_fields: Vec::new(),
        };

        assert!(super::ensure_legacy_bootstrap_supported(&config).is_ok());
    }

    #[test]
    fn resolves_default_and_profile_relative_sqlite_paths() {
        let root = test_root("sqlite-path");
        let paths = resolve_paths(Some(&root), None).expect("应能解析 Profile 路径");

        let default_config = ProfileConfig {
            storage: StorageDescriptor::default(),
            provider: crate::provider_config::ProviderDescriptor {
                provider: None,
                model: None,
                base_url: None,
                api_key_env: None,
                providers: std::collections::BTreeMap::new(),
            },
            workspace: crate::provider_config::WorkspaceDescriptor::default(),
            unknown_fields: Vec::new(),
        };
        assert_eq!(
            super::resolve_sqlite_database_path(&paths, &default_config).unwrap(),
            root.join("state.db")
        );

        let custom_config = ProfileConfig {
            storage: StorageDescriptor {
                path: Some("data/custom.db".into()),
                ..StorageDescriptor::default()
            },
            ..default_config
        };
        assert_eq!(
            super::resolve_sqlite_database_path(&paths, &custom_config).unwrap(),
            root.join("data/custom.db")
        );
        fs::remove_dir_all(root).expect("应能清理 storage path fixture");
    }

    #[test]
    fn resolves_remote_descriptor_without_opening_a_database() {
        let root = test_root("remote");
        fs::write(
            root.join("config.yaml"),
            "storage:\n  kind: remote\n  connection_env: SAGENT_DB_URL\n",
        )
        .expect("应能写入 storage descriptor 配置");
        let paths = resolve_paths(Some(&root), None).expect("应能解析 Profile 路径");

        // 远程 descriptor 只验证配置意图；解析过程不应因为旧 bootstrap 而创建 state.db。
        let config = load_profile_config(&paths).expect("应能加载 storage 快照");
        let descriptor = super::resolve_storage_descriptor_from_config(&config)
            .expect("descriptor 应能从快照解析");
        assert!(super::ensure_legacy_bootstrap_supported(&config).is_err());

        assert_eq!(descriptor.kind, StorageKind::Remote);
        assert_eq!(descriptor.connection_env.as_deref(), Some("SAGENT_DB_URL"));
        assert!(!root.join("state.db").exists());
        fs::remove_dir_all(root).expect("应能清理 storage descriptor fixture");
    }
}
