//! SQLite Session 全文搜索端口。
//!
//! 搜索能力单独实现为只读端口，使 FTS 查询可以通过 `ReadStorage.session` 暴露，而不
//! 让搜索调用方获得任何写入句柄。FTS 的语法校验和查询边界仍由底层搜索实现负责。
//!
//! 作者：SongZQ

use sagent_types::SearchHit;

use super::{SharedDatabase, lock_database};
use crate::{MessageSearchQuery, StorageResult, ports::SearchStorage};

/// SQLite Session 全文搜索端口，仅持有受保护的数据库资源。
pub(super) struct SqliteSearchStorage {
    pub(super) database: SharedDatabase,
}

impl SearchStorage for SqliteSearchStorage {
    fn search_messages(&self, query: &MessageSearchQuery) -> StorageResult<Vec<SearchHit>> {
        lock_database(&self.database)?.search_messages(query)
    }
}
