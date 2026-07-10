//! 日志系统初始化 — 所有入口（CLI / Gateway / 测试）共用。
//!
//! 基于 `tracing-subscriber`，支持环境变量 `RUST_LOG` 覆盖级别，
//! sagent 内部 crate 默认使用 `debug` 级别以便排查问题。

use tracing_subscriber::{fmt, EnvFilter};

/// 日志初始化配置。
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// 全局默认日志级别（未设置 `RUST_LOG` 时生效）
    pub default_level: String,
    /// sagent 内部 crate 的日志级别
    pub internal_level: String,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            default_level: "info".into(),
            internal_level: "debug".into(),
        }
    }
}

/// 使用默认配置初始化日志系统。
///
/// 等价于 `init_with_default("info")`。
///
/// # 示例
///
/// ```
/// sagent_core::logging::init_default();
/// ```
pub fn init_default() {
    init_with_default("info");
}

/// 使用自定义默认级别初始化日志系统（便捷方法）。
///
/// `RUST_LOG` 环境变量优先级更高，会覆盖传入的 `default_level`。
/// sagent 内部 crate 固定为 `debug` 级别。
///
/// # 参数
///
/// * `default_level` - 全局默认日志级别，如 `"info"`、`"debug"`、`"warn"`
///
/// # 示例
///
/// ```
/// // 生产环境用 info
/// sagent_core::logging::init_with_default("info");
/// // 开发调试用 debug
/// sagent_core::logging::init_with_default("debug");
/// ```
pub fn init_with_default(default_level: &str) {
    init(LogConfig {
        default_level: default_level.to_string(),
        ..Default::default()
    });
}

/// 使用完整配置初始化日志系统。
///
/// 优先级：`RUST_LOG` 环境变量 > `config` 参数。
/// 输出格式：带时间戳、target 名称，不含线程 ID / 文件名 / 行号。
///
/// # 参数
///
/// * `config` - 日志配置，见 [`LogConfig`]
///
/// # 示例
///
/// ```
/// use sagent_core::logging::{init, LogConfig};
/// init(LogConfig {
///     default_level: "warn".into(),
///     internal_level: "debug".into(),
/// });
/// ```
pub fn init(config: LogConfig) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "{},sagent={},sagent_core={},sagent_cli={},sagent_gateway={}",
            config.default_level,
            config.internal_level,
            config.internal_level,
            config.internal_level,
            config.internal_level,
        ))
    });

    fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .init();
}
