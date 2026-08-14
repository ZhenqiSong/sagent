//! 配置文件加载器。
//!
//! 缺少配置文件时返回默认快照；不会创建 home、配置文件或 secrets 文件。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 1 配置加载

use std::path::Path;

use serde_yaml::{Mapping, Value};

use crate::config::Config;
use crate::error::ConfigError;
use crate::paths::ConfigPaths;

/// 从 Sagent home 加载 YAML 配置。
#[derive(Debug, Clone)]
pub struct ConfigLoader {
    paths: ConfigPaths,
}

impl ConfigLoader {
    /// 使用显式路径创建加载器。
    pub fn new(paths: ConfigPaths) -> Self {
        Self { paths }
    }

    /// 从 `SAGENT_HOME` 或平台默认路径创建加载器。
    pub fn discover() -> Result<Self, ConfigError> {
        Ok(Self::new(ConfigPaths::discover()?))
    }

    /// 返回加载器使用的路径集合。
    pub fn paths(&self) -> &ConfigPaths {
        &self.paths
    }

    /// 读取配置文件；文件不存在时返回完整默认配置。
    pub fn load(&self) -> Result<Config, ConfigError> {
        let path = self.paths.config_file();
        if !path.exists() {
            return Ok(Config::default());
        }
        let content = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        self.load_yaml(&content)
    }

    /// 从 YAML 字符串构造配置快照。
    pub fn load_yaml(&self, content: &str) -> Result<Config, ConfigError> {
        let value: Value = serde_yaml::from_str(content).map_err(|_| ConfigError::Yaml {
            message: "无法解析 YAML".to_string(),
        })?;
        validate_document(&value)?;
        let mut config: Config = serde_yaml::from_value(value).map_err(|_| ConfigError::Yaml {
            message: "配置字段无法转换为目标类型".to_string(),
        })?;
        if let Some(path) = config.database.path.take() {
            config.database.path = Some(self.paths.resolve_database_path(path));
        }
        config.validate()?;
        Ok(config)
    }

    /// 从指定文件路径加载配置，仍使用当前 loader 的 home 解析相对数据库路径。
    pub fn load_file(&self, path: impl AsRef<Path>) -> Result<Config, ConfigError> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        self.load_yaml(&content)
    }
}

fn validate_document(value: &Value) -> Result<(), ConfigError> {
    let root = mapping(value, "<root>")?;
    validate_keys(
        root,
        "",
        &["version", "runtime", "database", "rpc", "logging"],
    )?;
    if let Some(value) = root.get("version") {
        unsigned(value, "version")?;
    }
    validate_section(
        root,
        "runtime",
        &[
            "shutdown_timeout_ms",
            "max_live_sessions",
            "actor_mailbox_capacity",
            "event_buffer_capacity",
        ],
        |map| {
            for key in [
                "shutdown_timeout_ms",
                "max_live_sessions",
                "actor_mailbox_capacity",
                "event_buffer_capacity",
            ] {
                if let Some(value) = map.get(key) {
                    unsigned(value, &format!("runtime.{key}"))?;
                }
            }
            Ok(())
        },
    )?;
    validate_section(
        root,
        "database",
        &["path", "busy_timeout_ms", "synchronous"],
        |map| {
            if let Some(value) = map.get("path") {
                if !value.is_null() && value.as_str().is_none() {
                    return Err(ConfigError::InvalidType {
                        key_path: "database.path".to_string(),
                        expected: "字符串或 null",
                    });
                }
            }
            if let Some(value) = map.get("busy_timeout_ms") {
                unsigned(value, "database.busy_timeout_ms")?;
            }
            if let Some(value) = map.get("synchronous") {
                string(value, "database.synchronous")?;
                enum_value(value, "database.synchronous", &["full", "normal", "off"])?;
            }
            Ok(())
        },
    )?;
    validate_section(
        root,
        "rpc",
        &["max_line_bytes", "max_response_bytes"],
        |map| {
            for key in ["max_line_bytes", "max_response_bytes"] {
                if let Some(value) = map.get(key) {
                    unsigned(value, &format!("rpc.{key}"))?;
                }
            }
            Ok(())
        },
    )?;
    validate_section(root, "logging", &["level"], |map| {
        if let Some(value) = map.get("level") {
            string(value, "logging.level")?;
            enum_value(
                value,
                "logging.level",
                &["trace", "debug", "info", "warn", "error"],
            )?;
        }
        Ok(())
    })?;
    Ok(())
}

fn validate_section<F>(
    root: &Mapping,
    section: &str,
    known: &[&str],
    validate_values: F,
) -> Result<(), ConfigError>
where
    F: FnOnce(&Mapping) -> Result<(), ConfigError>,
{
    let Some(value) = root.get(section) else {
        return Ok(());
    };
    let map = mapping(value, section)?;
    validate_keys(map, section, known)?;
    validate_values(map)
}

fn mapping<'a>(value: &'a Value, key_path: &str) -> Result<&'a Mapping, ConfigError> {
    value.as_mapping().ok_or_else(|| ConfigError::InvalidType {
        key_path: key_path.to_string(),
        expected: "对象",
    })
}

fn validate_keys(map: &Mapping, prefix: &str, known: &[&str]) -> Result<(), ConfigError> {
    for key in map.keys() {
        let Some(key) = key.as_str() else {
            return Err(ConfigError::InvalidType {
                key_path: if prefix.is_empty() {
                    "<root>".to_string()
                } else {
                    prefix.to_string()
                },
                expected: "字符串 key",
            });
        };
        if !known.contains(&key) {
            let key_path = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };
            return Err(ConfigError::UnknownKey { key_path });
        }
    }
    Ok(())
}

fn unsigned(value: &Value, key_path: &str) -> Result<(), ConfigError> {
    if value.as_u64().is_none() {
        return Err(ConfigError::InvalidType {
            key_path: key_path.to_string(),
            expected: "非负整数",
        });
    }
    Ok(())
}

fn string(value: &Value, key_path: &str) -> Result<(), ConfigError> {
    if value.as_str().is_none() {
        return Err(ConfigError::InvalidType {
            key_path: key_path.to_string(),
            expected: "字符串",
        });
    }
    Ok(())
}

fn enum_value(value: &Value, key_path: &str, allowed: &[&str]) -> Result<(), ConfigError> {
    let Some(value) = value.as_str() else {
        return Err(ConfigError::InvalidType {
            key_path: key_path.to_string(),
            expected: "字符串",
        });
    };
    if !allowed.contains(&value) {
        return Err(ConfigError::InvalidValue {
            key_path: key_path.to_string(),
            message: format!("只支持: {}", allowed.join(", ")),
        });
    }
    Ok(())
}
