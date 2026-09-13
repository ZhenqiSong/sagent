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

/// 只读调用所需的领域存储端口集合。
///
/// CLI 查询、RPC 事件补读和全文搜索不应获得 `SessionStorage` 写入能力；独立的只读
/// 聚合让后端可以使用只读连接、连接池中的只读事务或其它等价实现。
pub struct StorageReadDependencies {
    query: Box<dyn SessionQueryStorage>,
    search: Box<dyn SearchStorage>,
}

/// 仅包含会话写入能力的领域依赖集合。
///
/// 短生命周期的写命令不需要查询和全文搜索能力；单独的聚合可以让 `StorageManager`
/// 在申请依赖时表达最小权限。`SessionActor` 仍使用包含配套查询端口的
/// `StorageDependencies`，以保持其单写和读取上下文的一致性。
pub struct StorageWriteDependencies {
    session: Box<dyn SessionStorage>,
}

impl StorageWriteDependencies {
    /// 用一个会话写入端口创建最小可写依赖集合。
    pub fn from_session(session: Box<dyn SessionStorage>) -> Self {
        Self { session }
    }

    /// 取得可变会话写入端口；调用方负责维护其事务边界。
    pub fn session_mut(&mut self) -> &mut dyn SessionStorage {
        self.session.as_mut()
    }

    /// 拆出会话写入端口所有权，供领域对象接管其生命周期。
    pub fn into_session(self) -> Box<dyn SessionStorage> {
        self.session
    }
}

impl StorageReadDependencies {
    /// 用会话查询和全文搜索端口创建只读依赖集合。
    pub fn new<Q, H>(query: Q, search: H) -> Self
    where
        Q: SessionQueryStorage + 'static,
        H: SearchStorage + 'static,
    {
        Self {
            query: Box::new(query),
            search: Box::new(search),
        }
    }

    /// 取得会话与消息查询端口。
    pub fn query(&self) -> &dyn SessionQueryStorage {
        self.query.as_ref()
    }

    /// 取得全文搜索端口。
    pub fn search(&self) -> &dyn SearchStorage {
        self.search.as_ref()
    }

    /// 拆出只读端口所有权，供协议服务或工具在装配边界持有。
    pub fn into_parts(self) -> (Box<dyn SessionQueryStorage>, Box<dyn SearchStorage>) {
        (self.query, self.search)
    }
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

    /// 拆出三个端口的所有权，供 Actor 或 transport 在装配边界绑定各自职责。
    pub fn into_parts(
        self,
    ) -> (
        Box<dyn SessionStorage>,
        Box<dyn SessionQueryStorage>,
        Box<dyn SearchStorage>,
    ) {
        (self.session, self.query, self.search)
    }
}
