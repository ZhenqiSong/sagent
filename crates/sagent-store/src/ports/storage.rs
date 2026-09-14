//! 按业务领域组织的持久化能力聚合。
//!
//! 本模块只定义顶层能力对象，把会话写入、查询和搜索收敛为 `storage.session` 外观。
//! 具体方法分别位于同主题模块；这里不打开数据库、不执行 SQL，也不决定事务策略。
//! `StorageDependencies` 等旧类型仍位于迁移适配层，待上层调用方完成迁移后再删除。

use super::{
    ReadOnlySessionStorage, SessionQueryStorage, SessionStorage, SessionWriteStorage,
    StorageDependencies, StorageReadDependencies, StorageWriteDependencies,
    WriteOnlySessionStorage,
};

/// 一个 Profile 的完整业务存储能力。
///
/// `Storage` 只按业务域公开能力聚合；调用方通过 `storage.session` 访问会话、Turn、
/// 消息和搜索操作，不需要知道底层使用 SQLite 文件、远程连接还是连接池。完整对象
/// 通常交给单个 `SessionActor` 独占，以保持写入顺序和事务边界。
pub struct Storage {
    /// 当前 Profile 的会话业务存储外观。
    pub session: SessionStorage,
}

impl Storage {
    /// 使用已经组装好的业务域存储构造完整聚合。
    ///
    /// Session 底层端口、连接生命周期和迁移策略由 Manager/adapter 在构造
    /// `SessionStorage` 时决定，顶层聚合不再感知这些细节。
    pub fn new(session: SessionStorage) -> Self {
        Self { session }
    }

    /// 将兼容期依赖集合转换为新的业务聚合。
    pub fn from_dependencies(dependencies: StorageDependencies) -> Self {
        let (write, query, search) = dependencies.into_parts();
        Self::new(SessionStorage::from_parts(
            write,
            ReadOnlySessionStorage::from_parts(query, search),
        ))
    }

    /// 将新业务存储聚合转换为迁移期的旧端口依赖集合。
    ///
    /// Runtime 逐步从 `StorageFactory` 切换到 `StorageManager` 时，Actor 构造边界仍需
    /// 暂时接收旧集合；转换只拆解已经装配好的端口，不重新打开数据库或改变事务边界。
    pub fn into_dependencies(self) -> StorageDependencies {
        let (write, query, search) = self.into_parts();
        StorageDependencies::from_parts(write, query, search)
    }

    /// 拆出底层端口，供迁移适配层或测试装配边界接管所有权。
    pub fn into_parts(
        self,
    ) -> (
        Box<dyn SessionWriteStorage>,
        Box<dyn SessionQueryStorage>,
        Box<dyn super::SearchStorage>,
    ) {
        self.session.into_parts()
    }
}

/// 为迁移期 Actor 构造边界提供从新业务聚合到旧端口集合的标准转换。
impl From<Storage> for StorageDependencies {
    fn from(storage: Storage) -> Self {
        storage.into_dependencies()
    }
}

/// 只读 Profile 存储能力，只公开 Session 查询和搜索。
pub struct ReadStorage {
    /// 只读会话业务存储外观。
    pub session: ReadOnlySessionStorage,
}

impl ReadStorage {
    /// 使用已经组装好的只读业务域存储构造只读聚合。
    ///
    /// 查询端口和搜索端口的具体实现由 Manager/adapter 决定，不在顶层读对象中再次
    /// 暴露或组合。
    pub fn new(session: ReadOnlySessionStorage) -> Self {
        Self { session }
    }

    /// 将兼容期只读依赖转换为新的只读业务聚合。
    pub fn from_dependencies(dependencies: StorageReadDependencies) -> Self {
        let (query, search) = dependencies.into_parts();
        Self::new(ReadOnlySessionStorage::from_parts(query, search))
    }

    /// 拆出只读端口，供迁移适配层或测试装配边界接管所有权。
    pub fn into_parts(self) -> (Box<dyn SessionQueryStorage>, Box<dyn super::SearchStorage>) {
        self.session.into_parts()
    }
}

/// 只写 Profile 存储能力，只公开 Session 的状态变更操作。
pub struct WriteStorage {
    /// 只写会话业务存储外观。
    pub session: WriteOnlySessionStorage,
}

impl WriteStorage {
    /// 使用已经组装好的只写业务域存储构造最小可写聚合。
    pub fn new(session: WriteOnlySessionStorage) -> Self {
        Self { session }
    }

    /// 将兼容期可写依赖转换为新的只写业务聚合。
    pub fn from_dependencies(dependencies: StorageWriteDependencies) -> Self {
        Self::new(WriteOnlySessionStorage::from_parts(
            dependencies.into_session(),
        ))
    }

    /// 拆出写入端口，供迁移适配层或测试装配边界接管所有权。
    pub fn into_session(self) -> Box<dyn SessionWriteStorage> {
        self.session.into_session()
    }
}
