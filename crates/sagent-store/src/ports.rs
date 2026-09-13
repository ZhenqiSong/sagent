//! 面向业务领域的持久化端口。
//!
//! 端口只描述 Runtime、RPC、CLI 和工具需要的行为，不包含 SQL、连接、文件路径或
//! 具体数据库类型。不同后端应在各自 adapter 中实现这些端口。

mod dependencies;
mod query;
mod search;
mod session;

pub use dependencies::StorageDependencies;
pub use query::SessionQueryStorage;
pub use search::SearchStorage;
pub use session::SessionStorage;

/// 存储端口使用的统一结果类型。
///
/// 暂时保留现有错误链，后续 StorageError 收口时只需替换此别名及 adapter 映射。
pub type StorageResult<T> = anyhow::Result<T>;
