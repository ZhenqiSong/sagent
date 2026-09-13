//! Profile 作用域的存储管理端口。
//!
//! `StorageManager` 是后端资源与领域端口之间的窄边界。Bootstrap 只需创建并持有一个
//! manager，Runtime、RPC、CLI 和工具按职责申请短生命周期依赖，不需要知道 SQLite
//! 路径、PostgreSQL 连接池或其它后端资源的生命周期。
//!
//! 作者：SongZQ

use super::{ReadStorage, Storage, StorageResult, WriteStorage};

/// 管理单个 Profile 存储资源并按职责提供业务存储聚合。
///
/// 管理器本身可以被多个运行时组件共享，但每次申请都必须返回独立的存储对象：Actor
/// 的写入端口不能跨 Actor 共享，短操作也不能意外持有 manager 内部连接。Manager 不
/// 负责 Session/Turn CRUD；具体后端在 adapter 内部组装业务 Storage。
pub trait StorageManager: Send + Sync {
    /// 为一个 `SessionActor` 创建独占的完整业务存储。
    fn open_actor_storage(&self) -> StorageResult<Storage>;

    /// 创建只读业务存储，不得创建数据库或执行写入迁移。
    fn open_read_storage(&self) -> StorageResult<ReadStorage>;

    /// 为短生命周期写命令申请最小可写业务存储。
    fn open_write_storage(&self) -> StorageResult<WriteStorage>;

    /// 初始化当前 Profile 的后端资源并执行必要的 schema migration。
    ///
    /// 该入口明确表示可能创建数据库和修改 schema，只能由 bootstrap 或显式写入流程
    /// 调用；普通查询应使用 `open_read_storage` 或 `health_check`，不能借此初始化。
    fn initialize(&self) -> StorageResult<()>;

    /// 检查当前 Profile 的后端是否可用。
    ///
    /// 健康检查必须是只读、无副作用的操作；缺失数据库、schema 损坏或连接不可用时
    /// 返回错误，不能通过隐式创建数据库或 migration 掩盖问题。
    fn health_check(&self) -> StorageResult<()>;
}
