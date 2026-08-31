//! 基于 SQLite FTS5 的消息全文检索。
//!
//! 作者：SongZQ

use anyhow::{Context, Result};
use rusqlite::{params_from_iter, types::Value};
use sagent_types::{MessageId, SearchHit, SessionId};

use crate::Store;

/// 消息全文搜索的范围与可见性条件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageSearchQuery {
    /// 传给 FTS5 MATCH 的查询表达式，例如 "Rust AND SQLite"。
    pub query: String,
    /// 限定在某个会话内搜索；None 表示搜索全部会话。
    pub session_id: Option<SessionId>,
    /// 是否包含撤回、重试前等非活动消息；默认只包含活动或压缩历史消息。
    pub include_inactive: bool,
    /// 最多返回的命中数量。
    pub limit: u32,
}

impl MessageSearchQuery {
    /// 创建一个使用默认分页限制的搜索条件。
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            session_id: None,
            include_inactive: false,
            limit: 20,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.query.trim().is_empty() {
            anyhow::bail!("全文搜索词不能为空");
        }
        if self.limit == 0 {
            anyhow::bail!("全文搜索 limit 必须大于零");
        }
        Ok(())
    }
}

/// FTS5 表的 rowid 对应 messages.id，因此可安全映射为消息命中。
fn map_search_hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchHit> {
    Ok(SearchHit {
        session_id: SessionId::new(row.get::<_, String>(0)?),
        message_id: Some(MessageId::new(row.get(1)?)),
        snippet: row.get(2)?,
        rank: Some(row.get(3)?),
    })
}

/// 判断查询是否包含 CJK 统一表意文字。
///
/// SQLite 默认的 unicode61 tokenizer 不会把连续中文拆成可供子串 MATCH 的词元；
/// 对此类查询改用参数绑定的 `LIKE`，避免出现“库里有中文但搜索不到”的体验。
fn contains_cjk(query: &str) -> bool {
    query.chars().any(|character| {
        matches!(
            character as u32,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
        )
    })
}

impl Store {
    /// 在 messages_fts 索引中搜索消息内容。
    ///
    /// 默认显示活动消息和压缩保留的历史消息，隐藏普通撤回消息；这与会话搜索
    /// 的可见性规则一致。命中按 FTS5 的 bm25 相关度排序。
    pub fn search_messages(&self, query: &MessageSearchQuery) -> Result<Vec<SearchHit>> {
        query.validate()?;

        let visibility_clause = if query.include_inactive {
            "1 = 1"
        } else {
            "(m.active = 1 OR m.compacted = 1)"
        };
        let (mut sql, mut parameters) = if contains_cjk(&query.query) {
            (
                format!(
                    "SELECT m.session_id, m.id, m.content, 0.0
                     FROM messages m
                     WHERE m.content LIKE '%' || ? || '%' AND {visibility_clause}"
                ),
                vec![Value::Text(query.query.clone())],
            )
        } else {
            (
                format!(
                    "SELECT m.session_id, m.id,
                            snippet(messages_fts, 0, '[', ']', '…', 16),
                            bm25(messages_fts)
                     FROM messages_fts
                     INNER JOIN messages m ON m.id = messages_fts.rowid
                     WHERE messages_fts MATCH ? AND {visibility_clause}"
                ),
                vec![Value::Text(query.query.clone())],
            )
        };
        if let Some(session_id) = &query.session_id {
            sql.push_str(" AND m.session_id = ?");
            parameters.push(Value::Text(session_id.as_str().to_owned()));
        }
        sql.push_str(if contains_cjk(&query.query) {
            " ORDER BY m.id DESC LIMIT ?"
        } else {
            " ORDER BY bm25(messages_fts), m.id DESC LIMIT ?"
        });
        parameters.push(Value::Integer(i64::from(query.limit)));

        let mut statement = self
            .connection
            .prepare(&sql)
            .context("准备 FTS5 消息搜索失败；数据库可能未建立 messages_fts 索引")?;
        let rows = statement
            .query_map(params_from_iter(parameters.iter()), map_search_hit)
            .context("执行 FTS5 消息搜索失败")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("读取 FTS5 搜索结果失败")
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use rusqlite::Connection;
    use sagent_types::SessionId;

    use super::{MessageSearchQuery, Store};

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sagent-search-{name}-{}.db", std::process::id()))
    }

    fn remove(path: &std::path::Path) {
        let _ = fs::remove_file(path);
    }

    fn create_fixture(path: &std::path::Path) {
        let connection = Connection::open(path).expect("应能创建测试数据库");
        connection
            .execute_batch(
                "CREATE TABLE messages (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    content TEXT NOT NULL,
                    active INTEGER NOT NULL,
                    compacted INTEGER NOT NULL
                 );
                 CREATE VIRTUAL TABLE messages_fts USING fts5(content);
                 INSERT INTO messages VALUES
                    (1, 'session-1', 'Rust 和 SQLite', 1, 0),
                    (2, 'session-1', 'Rust 已撤回', 0, 0),
                    (3, 'session-1', 'Rust 压缩历史', 0, 1),
                    (4, 'session-2', 'Python 迁移到 Rust', 1, 0),
                    (5, 'session-2', '中文消息与 emoji 🚀', 1, 0);
                 INSERT INTO messages_fts(rowid, content) VALUES
                    (1, 'Rust 和 SQLite'),
                    (2, 'Rust 已撤回'),
                    (3, 'Rust 压缩历史'),
                    (4, 'Python 迁移到 Rust'),
                    (5, '中文消息与 emoji 🚀');",
            )
            .expect("应能创建 FTS5 测试数据");
    }

    fn create_cjk_fixture(path: &std::path::Path) {
        let connection = Connection::open(path).expect("应能创建 CJK fixture 数据库");
        connection
            .execute_batch(include_str!("../tests/fixtures/cjk_emoji_fts.sql"))
            .expect("CJK fixture SQL 应能执行");
    }

    fn hit_ids(hits: &[sagent_types::SearchHit]) -> Vec<i64> {
        let mut ids: Vec<_> = hits
            .iter()
            .map(|hit| hit.message_id.as_ref().expect("消息搜索应有 ID").get())
            .collect();
        ids.sort_unstable();
        ids
    }

    #[test]
    fn search_includes_active_and_compacted_messages_by_default() {
        let path = test_path("visibility");
        remove(&path);
        create_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");

        let hits = store
            .search_messages(&MessageSearchQuery::new("Rust"))
            .expect("应能搜索 FTS5 索引");

        assert_eq!(hit_ids(&hits), vec![1, 3, 4]);
        assert!(hits.iter().all(|hit| hit.snippet.contains('[')));
        remove(&path);
    }

    #[test]
    fn search_can_be_scoped_to_one_session() {
        let path = test_path("session");
        remove(&path);
        create_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");
        let mut query = MessageSearchQuery::new("Rust");
        query.session_id = Some(SessionId::new("session-1"));

        let hits = store.search_messages(&query).expect("应能按会话搜索");

        assert_eq!(hit_ids(&hits), vec![1, 3]);
        assert!(
            hits.iter()
                .all(|hit| hit.session_id.as_str() == "session-1")
        );
        remove(&path);
    }

    #[test]
    fn inactive_search_includes_soft_deleted_messages() {
        let path = test_path("inactive");
        remove(&path);
        create_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");
        let mut query = MessageSearchQuery::new("Rust");
        query.include_inactive = true;

        let hits = store
            .search_messages(&query)
            .expect("审计搜索应包含撤回消息");

        assert_eq!(hit_ids(&hits), vec![1, 2, 3, 4]);
        remove(&path);
    }

    #[test]
    fn empty_or_invalid_fts_query_returns_error() {
        let path = test_path("invalid");
        remove(&path);
        create_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");

        assert!(
            store
                .search_messages(&MessageSearchQuery::new("   "))
                .is_err()
        );
        assert!(
            store
                .search_messages(&MessageSearchQuery::new("\""))
                .is_err()
        );
        remove(&path);
    }

    #[test]
    fn search_handles_cjk_and_emoji_content_without_panicking() {
        let path = test_path("unicode");
        remove(&path);
        create_cjk_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");

        let hits = store
            .search_messages(&MessageSearchQuery::new("中文消息"))
            .expect("中文 FTS5 查询不应失败");

        assert_eq!(hit_ids(&hits), vec![1]);
        assert!(hits[0].snippet.contains('🚀'));
        remove(&path);
    }
}
