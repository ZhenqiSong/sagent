//! Sagent 的最小工具契约与注册表。
//!
//! 本 crate 负责定义工具元数据、执行安全的 workspace 文件读取与受监管 terminal，
//! 并生成稳定的模型 schema。
//! Store 写入、RuntimeEvent 和 SessionActor 生命周期属于上层，不能由工具实现直接触碰。

mod command_policy;
mod definition;
mod error;
mod process;
mod read_file;
mod registry;
mod result;
mod schema;
mod terminal;
mod workspace;

pub use command_policy::{CommandRisk, classify_command};
pub use definition::{ToolDefinition, ToolPermission};
pub use error::RegistryError;
pub use process::{
    BoundedOutput, ProcessSupervisor, ProcessTreeGuard, TerminationReason, sanitize_environment,
};
pub use read_file::{ReadFileLimits, ReadFileRequest, ReadFileService};
pub use registry::ToolRegistry;
pub use result::{TRUNCATION_MARKER, ToolResult};
pub use schema::{canonical_json, canonical_tool_schema, tool_schema_hash};
pub use terminal::{TerminalExecutor, TerminalLimits, TerminalRequest};
pub use workspace::{WorkspaceError, WorkspaceRoot};
