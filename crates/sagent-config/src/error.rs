//! 配置错误类型。
//!
//! 错误只包含安全的 key path 和概要信息，不包含完整 YAML 或敏感字段值。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 1 配置错误

use std::path::PathBuf;

use crate::paths::PathError;

/// 配置加载和校验错误。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Sagent home 路径无效。
    #[error("配置 home 路径无效: {0}")]
    Path(#[from] PathError),
    /// 读取配置文件失败。
    #[error("读取配置文件失败: {path}: {source}")]
    Io {
        /// 配置文件路径。
        path: PathBuf,
        /// 底层 I/O 错误。
        #[source]
        source: std::io::Error,
    },
    /// YAML 结构或语法错误。
    #[error("配置 YAML 无效: {message}")]
    Yaml {
        /// 不含原始文件内容的概要信息。
        message: String,
    },
    /// 配置字段类型错误。
    #[error("配置字段类型错误: {key_path}，期望 {expected}")]
    InvalidType {
        /// 点号分隔的字段路径。
        key_path: String,
        /// 期望的安全类型描述。
        expected: &'static str,
    },
    /// 出现未声明的配置字段。
    #[error("未知配置字段: {key_path}")]
    UnknownKey {
        /// 点号分隔的字段路径。
        key_path: String,
    },
    /// 字段值超出约束。
    #[error("配置字段值无效: {key_path}，{message}")]
    InvalidValue {
        /// 点号分隔的字段路径。
        key_path: String,
        /// 不包含原始敏感值的概要信息。
        message: String,
    },
}
