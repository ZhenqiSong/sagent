//! 面向业务领域的持久化端口。
//!
//! 端口只描述 Runtime、RPC、CLI 和工具需要的行为，不包含 SQL、连接、文件路径或
//! 具体数据库类型。不同后端应在各自 adapter 中实现这些端口。

mod manager;
mod query;
mod read_storage;
mod search;
mod session;
mod session_storage;
mod storage;
mod write_storage;

#[cfg(test)]
mod storage_tests;

pub use manager::StorageManager;
pub use query::SessionQueryStorage;
pub use read_storage::ReadOnlySessionStorage;
pub use search::SearchStorage;
pub use session::SessionWriteStorage;
pub use session_storage::SessionStorage;
pub use storage::{ReadStorage, Storage, WriteStorage};
pub use write_storage::WriteOnlySessionStorage;

/// 存储端口使用的统一结果类型。
///
/// 暂时保留现有错误链，后续 StorageError 收口时只需替换此别名及 adapter 映射。
pub type StorageResult<T> = anyhow::Result<T>;
