//! 存储后端的依赖创建端口。

use super::{StorageDependencies, StorageResult};

/// 根据已冻结的后端意图创建一组独立存储依赖。
///
/// Factory 本身可以由 Runtime、RPC 或 CLI 共享，但每次 `create` 都必须返回新的
/// `StorageDependencies`。这样每个 SessionActor 都拥有自己的写入端口，后端可以在
/// adapter 内决定连接、连接池或事务的生命周期，而不会把这些细节泄漏到上层。
pub trait StorageFactory: Send + Sync {
    /// 创建一个可供单个运行单元使用的领域存储依赖集合。
    ///
    /// 实现必须根据创建 Factory 时绑定的 descriptor 选择后端；不支持的后端应返回
    /// 明确错误，不能静默切换到 SQLite 或其它默认实现。
    fn create(&self) -> StorageResult<StorageDependencies>;
}
