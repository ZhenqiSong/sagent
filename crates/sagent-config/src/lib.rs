//! sagent-config — Phase 1 配置加载和路径解析。
//!
//! 本 crate 只负责读取不可变配置快照，不读取 secrets，不依赖 Runtime、Session 或 CLI。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 1 配置模块

pub mod config;
pub mod defaults;
pub mod error;
pub mod loader;
pub mod paths;

pub use config::{
    Config, DatabaseConfig, LogLevel, LoggingConfig, RpcConfig, RuntimeConfig, SynchronousMode,
};
pub use error::ConfigError;
pub use loader::ConfigLoader;
pub use paths::{ConfigPaths, PathError};
