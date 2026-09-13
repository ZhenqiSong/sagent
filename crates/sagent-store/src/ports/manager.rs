//! Profile 作用域的存储管理端口。
//!
//! `StorageManager` 是后端资源与领域端口之间的窄边界。Bootstrap 只需创建并持有一个
//! manager，Runtime、RPC、CLI 和工具按职责申请短生命周期依赖，不需要知道 SQLite
//! 路径、PostgreSQL 连接池或其它后端资源的生命周期。
//!
//! 作者：SongZQ

use super::{
    SearchStorage, StorageDependencies, StorageReadDependencies, StorageResult,
    StorageWriteDependencies,
};

/// 管理单个 Profile 存储资源并按职责提供领域依赖。
///
/// 管理器本身可以被多个运行时组件共享，但每次申请都必须返回独立的依赖聚合：Actor
/// 的写入端口不能跨 Actor 共享，短操作也不能意外持有 manager 内部连接。具体后端可
/// 覆盖默认的窄入口，以便使用连接池或专用事务策略。
pub trait StorageManager: Send + Sync {
    /// 为一个 `SessionActor` 创建独占写入端口及配套查询、搜索端口。
    fn open_actor_storage(&self) -> StorageResult<StorageDependencies>;

    /// 创建只读查询与搜索端口，不得创建数据库或执行写入迁移。
    fn open_read_storage(&self) -> StorageResult<StorageReadDependencies>;

    /// 为短生命周期写命令申请最小可写依赖。
    ///
    /// 默认实现从一组 Actor 依赖中丢弃查询和搜索端口；后端若能直接申请写端口，可
    /// 覆盖此方法避免无用端口初始化。
    fn open_write_storage(&self) -> StorageResult<StorageWriteDependencies> {
        let (session, _query, _search) = self.open_actor_storage()?.into_parts();
        Ok(StorageWriteDependencies::from_session(session))
    }

    /// 单独申请全文搜索端口，避免搜索工具获得写入能力。
    ///
    /// 默认实现复用只读依赖并释放查询端口；连接池型后端可以覆盖该方法使用更轻量
    /// 的搜索句柄。
    fn open_search_storage(&self) -> StorageResult<Box<dyn SearchStorage>> {
        let (_query, search) = self.open_read_storage()?.into_parts();
        Ok(search)
    }
}
