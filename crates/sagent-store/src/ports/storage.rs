//! 按业务领域组织的持久化能力聚合。
//!
//! 本模块只定义顶层能力对象，把会话写入、查询和搜索收敛为 `storage.session` 外观。
//! 具体方法分别位于同主题模块；这里不打开数据库、不执行 SQL，也不决定事务策略。
use super::{
    ReadOnlySessionStorage, SessionQueryStorage, SessionStorage, SessionWriteStorage,
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

    /// 拆出底层端口，供 Runtime Actor 和协议适配边界接管所有权。
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

    /// 拆出只读端口，供 Runtime 查询适配和测试装配边界接管所有权。
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

    /// 拆出写入端口，供迁移适配层或测试装配边界接管所有权。
    pub fn into_session(self) -> Box<dyn SessionWriteStorage> {
        self.session.into_session()
    }
}
