//! 存储端口的运行时依赖聚合。

use super::{SearchStorage, SessionQueryStorage, SessionStorage};

/// 一次运行时装配所需的领域存储依赖。
///
/// 该聚合只保存端口对象，不保存数据库路径、连接、连接池或后端配置。Factory 负责
/// 根据 `StorageDescriptor` 创建它；上层只能通过领域端口访问存储行为。
pub struct StorageDependencies {
    session: Box<dyn SessionStorage>,
    query: Box<dyn SessionQueryStorage>,
    search: Box<dyn SearchStorage>,
}

impl StorageDependencies {
    /// 用会话写入、会话查询和全文搜索三个领域端口创建依赖聚合。
    pub fn new<S, Q, H>(session: S, query: Q, search: H) -> Self
    where
        S: SessionStorage + 'static,
        Q: SessionQueryStorage + 'static,
        H: SearchStorage + 'static,
    {
        Self {
            session: Box::new(session),
            query: Box::new(query),
            search: Box::new(search),
        }
    }

    /// 取得只读的会话写入端口。
    pub fn session(&self) -> &dyn SessionStorage {
        self.session.as_ref()
    }

    /// 取得可变的会话写入端口；Actor 应是该端口的唯一写入者。
    pub fn session_mut(&mut self) -> &mut dyn SessionStorage {
        self.session.as_mut()
    }

    /// 取得会话与消息查询端口。
    pub fn query(&self) -> &dyn SessionQueryStorage {
        self.query.as_ref()
    }

    /// 取得全文搜索端口。
    pub fn search(&self) -> &dyn SearchStorage {
        self.search.as_ref()
    }
}
