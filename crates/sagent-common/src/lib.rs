//! sagent-common — 共享类型与工具函数。
//!
//! 本 crate 零 IO 依赖，仅包含纯数据结构和基础工具函数，
//! 所有 crate 均可安全依赖。

pub mod error;
pub mod types;

// Re-export 常用类型
pub use error::SagentError;
pub use types::{ContentBlock, Message, Role};
