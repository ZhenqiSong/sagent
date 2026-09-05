//! Sagent 的最小工具契约与注册表。
//!
//! 本 crate 只负责定义工具元数据、校验注册行为并生成稳定的模型 schema。
//! 工具执行、Store 写入、RuntimeEvent 和 SessionActor 生命周期属于后续层，
//! 因此这里不依赖 SQLite、Tokio 或 `sagent-runtime`。

mod definition;
mod error;
mod registry;
mod result;
mod schema;

pub use definition::{ToolDefinition, ToolPermission};
pub use error::RegistryError;
pub use registry::ToolRegistry;
pub use result::{TRUNCATION_MARKER, ToolResult};
pub use schema::{canonical_json, canonical_tool_schema, tool_schema_hash};
