//! 消息全文搜索端口。

use sagent_types::SearchHit;

use crate::MessageSearchQuery;

use super::StorageResult;

/// 消息全文搜索的最小领域端口。
pub trait SearchStorage: Send + Sync {
    /// 按查询条件读取有界消息命中结果。
    fn search_messages(&self, query: &MessageSearchQuery) -> StorageResult<Vec<SearchHit>>;
}
