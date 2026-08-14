//! 配置数据模型和内存校验。
//!
//! 配置模型不包含 secret；加载后产生的值是一个独立快照，文件变化不会修改已有实例。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 1 配置模型

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::defaults;
use crate::error::ConfigError;

/// SQLite synchronous 配置。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SynchronousMode {
    /// 完整同步，优先保证数据持久性。
    #[default]
    Full,
    /// 平衡持久性和性能。
    Normal,
    /// 关闭同步，仅允许显式风险配置使用。
    Off,
}

/// 日志级别。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// 跟踪日志。
    Trace,
    /// 调试日志。
    Debug,
    /// 信息日志。
    #[default]
    Info,
    /// 警告日志。
    Warn,
    /// 错误日志。
    Error,
}

/// Runtime 运行限制。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RuntimeConfig {
    /// 优雅关闭超时时间（毫秒）。
    pub shutdown_timeout_ms: u64,
    /// 最大活跃 Session 数量。
    pub max_live_sessions: u32,
    /// 单个 Actor mailbox 容量。
    pub actor_mailbox_capacity: u32,
    /// 单个事件流缓冲容量。
    pub event_buffer_capacity: u32,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout_ms: defaults::SHUTDOWN_TIMEOUT_MS,
            max_live_sessions: defaults::MAX_LIVE_SESSIONS,
            actor_mailbox_capacity: defaults::ACTOR_MAILBOX_CAPACITY,
            event_buffer_capacity: defaults::EVENT_BUFFER_CAPACITY,
        }
    }
}

/// 数据库配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DatabaseConfig {
    /// 数据库路径；相对路径在加载时相对于 Sagent home 解析。
    pub path: Option<PathBuf>,
    /// SQLite busy timeout（毫秒）。
    pub busy_timeout_ms: u64,
    /// SQLite synchronous 模式。
    pub synchronous: SynchronousMode,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: None,
            busy_timeout_ms: defaults::BUSY_TIMEOUT_MS,
            synchronous: SynchronousMode::Full,
        }
    }
}

impl DatabaseConfig {
    /// 校验数据库路径、busy timeout 和 synchronous 设置。
    pub fn validate(&self) -> Result<(), ConfigError> {
        check_timeout("database.busy_timeout_ms", self.busy_timeout_ms)?;
        if let Some(path) = &self.path {
            if path.as_os_str().is_empty() {
                return Err(ConfigError::InvalidValue {
                    key_path: "database.path".to_string(),
                    message: "路径不能为空".to_string(),
                });
            }
            if path.to_string_lossy().contains('\0') {
                return Err(ConfigError::InvalidValue {
                    key_path: "database.path".to_string(),
                    message: "路径不能包含 NUL 字符".to_string(),
                });
            }
        }
        Ok(())
    }
}

/// JSON-RPC 限制。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RpcConfig {
    /// 单行请求大小上限（字节）。
    pub max_line_bytes: u64,
    /// 单个响应大小上限（字节）。
    pub max_response_bytes: u64,
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            max_line_bytes: defaults::MAX_LINE_BYTES,
            max_response_bytes: defaults::MAX_RESPONSE_BYTES,
        }
    }
}

/// 日志配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LoggingConfig {
    /// tracing 日志级别。
    pub level: LogLevel,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
        }
    }
}

/// 完整的不可变配置快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// 配置 schema 版本。
    pub version: u32,
    /// Runtime 限制。
    pub runtime: RuntimeConfig,
    /// 数据库设置。
    pub database: DatabaseConfig,
    /// JSON-RPC 限制。
    pub rpc: RpcConfig,
    /// 日志设置。
    pub logging: LoggingConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: defaults::CONFIG_VERSION,
            runtime: RuntimeConfig::default(),
            database: DatabaseConfig::default(),
            rpc: RpcConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

impl Config {
    /// 校验版本和所有数值边界。
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.version != defaults::CONFIG_VERSION {
            return Err(ConfigError::InvalidValue {
                key_path: "version".to_string(),
                message: format!("只支持版本 {}", defaults::CONFIG_VERSION),
            });
        }
        check_timeout(
            "runtime.shutdown_timeout_ms",
            self.runtime.shutdown_timeout_ms,
        )?;
        check_range(
            "runtime.max_live_sessions",
            self.runtime.max_live_sessions as u64,
            defaults::MAX_LIVE_SESSIONS_LIMIT as u64,
        )?;
        check_buffer(
            "runtime.actor_mailbox_capacity",
            self.runtime.actor_mailbox_capacity,
        )?;
        check_buffer(
            "runtime.event_buffer_capacity",
            self.runtime.event_buffer_capacity,
        )?;
        self.database.validate()?;
        check_range(
            "rpc.max_line_bytes",
            self.rpc.max_line_bytes,
            defaults::MAX_LINE_BYTES_LIMIT,
        )?;
        check_range(
            "rpc.max_response_bytes",
            self.rpc.max_response_bytes,
            defaults::MAX_RESPONSE_BYTES_LIMIT,
        )?;
        Ok(())
    }
}

fn check_timeout(key_path: &str, value: u64) -> Result<(), ConfigError> {
    check_range(key_path, value, defaults::MAX_TIMEOUT_MS)
}

fn check_buffer(key_path: &str, value: u32) -> Result<(), ConfigError> {
    check_range(key_path, value as u64, defaults::MAX_BUFFER_CAPACITY as u64)
}

fn check_range(key_path: &str, value: u64, max: u64) -> Result<(), ConfigError> {
    if !(defaults::MIN_POSITIVE..=max).contains(&value) {
        return Err(ConfigError::InvalidValue {
            key_path: key_path.to_string(),
            message: format!("必须在 {} 到 {} 之间", defaults::MIN_POSITIVE, max),
        });
    }
    Ok(())
}
