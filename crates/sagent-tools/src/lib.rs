//! Sagent 的最小工具契约与注册表。
//!
//! 本 crate 负责定义工具元数据、执行安全的 workspace 文件读取并生成稳定的模型 schema。
//! Store 写入、RuntimeEvent 和 SessionActor 生命周期属于上层，不能由工具实现直接触碰。

mod definition;
mod error;
mod read_file;
mod registry;
mod result;
mod schema;
mod workspace;

pub use definition::{ToolDefinition, ToolPermission};
pub use error::RegistryError;
pub use read_file::{ReadFileLimits, ReadFileRequest, ReadFileService};
pub use registry::ToolRegistry;
pub use result::{TRUNCATION_MARKER, ToolResult};
pub use schema::{canonical_json, canonical_tool_schema, tool_schema_hash};
pub use workspace::{WorkspaceError, WorkspaceRoot};
