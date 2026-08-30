//! 消息历史的只读查询。
//!
//! 作者：SongZQ

use std::collections::{HashMap, hash_map::Entry};

use anyhow::{Context, Result};
use rusqlite::{Connection, Row, params_from_iter, types::Value};
use sagent_types::{MessageId, SessionId, StoredMessage};

use crate::Store;

/// 读取一段会话消息时使用的筛选与分页条件。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageQuery {
    /// 是否包含回退操作软删除的消息。
    pub include_inactive: bool,
    /// 是否包含上下文压缩留下的展示历史，并去除跨压缩代的重复副本。
    pub include_compacted: bool,
    /// 单页最大消息数；None 表示不限制数量。
    pub limit: Option<u32>,
    /// 页内跳过的消息数，从零开始。
    pub offset: u32,
    /// 是否从最新消息向前取页，但最终仍按正序返回。
    pub latest: bool,
    /// 基于自增消息 ID 的游标；适合大历史记录的连续加载。
    pub after_id: Option<MessageId>,
}

/// 围绕某条消息加载的局部会话上下文。
#[derive(Clone, Debug)]
pub struct MessageWindow {
    /// 包含锚点的消息窗口，始终按消息 ID 正序排列。
    pub messages: Vec<StoredMessage>,
    /// 当前窗口中位于锚点之前的消息数量；小于请求窗口时表示已到会话开头。
    pub messages_before: u32,
    /// 当前窗口中位于锚点之后的消息数量；小于请求窗口时表示已到会话末尾。
    pub messages_after: u32,
}

impl MessageQuery {
    /// 验证无法同时成立的分页组合。
    fn validate(&self) -> Result<()> {
        if self.after_id.is_some() && (self.latest || self.offset != 0) {
            anyhow::bail!("after_id 不能与 latest 或 offset 同时使用");
        }
        if self.after_id.is_some() && self.include_compacted {
            anyhow::bail!("after_id 不能与 include_compacted 同时使用");
        }
        Ok(())
    }
}

/// 将固定投影的消息行转换为公共类型。
pub(crate) fn map_stored_message(row: &Row<'_>) -> rusqlite::Result<StoredMessage> {
    Ok(StoredMessage {
        id: MessageId::new(row.get(0)?),
        session_id: SessionId::new(row.get::<_, String>(1)?),
        role: row.get(2)?,
        content: row.get(3)?,
        timestamp: row.get(4)?,
        tool_call_id: row.get(5)?,
        tool_name: row.get(6)?,
        tool_calls: row.get(7)?,
        reasoning: row.get(8)?,
        finish_reason: row.get(9)?,
        display_kind: row.get(10)?,
        display_metadata: row.get(11)?,
        active: row.get::<_, i64>(12)? != 0,
        compacted: row.get::<_, i64>(13)? != 0,
    })
}

/// 执行不含压缩展示去重的消息查询。
fn query_messages(
    connection: &Connection,
    session_id: &SessionId,
    visibility_clause: &str,
    after_id: Option<&MessageId>,
    descending: bool,
    limit: Option<u32>,
    offset: u32,
) -> Result<Vec<StoredMessage>> {
    let order = if descending { "DESC" } else { "ASC" };
    let mut sql = format!(
        "SELECT id, session_id, role, COALESCE(content, ''), timestamp,
                tool_call_id, tool_name, tool_calls, reasoning, finish_reason,
                display_kind, display_metadata, active, compacted
         FROM messages
         WHERE session_id = ? AND {visibility_clause}"
    );
    let mut parameters = vec![Value::Text(session_id.as_str().to_owned())];

    if let Some(after_id) = after_id {
        sql.push_str(" AND id > ?");
        parameters.push(Value::Integer(after_id.get()));
    }

    sql.push_str(&format!(" ORDER BY id {order}"));
    if limit.is_some() || offset != 0 {
        sql.push_str(" LIMIT ? OFFSET ?");
        parameters.push(Value::Integer(limit.map_or(-1, i64::from)));
        parameters.push(Value::Integer(i64::from(offset)));
    }

    let mut statement = connection.prepare(&sql).context("准备消息查询失败")?;
    let rows = statement
        .query_map(params_from_iter(parameters.iter()), map_stored_message)
        .context("执行消息查询失败")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("读取消息记录失败")
}

/// 查询锚点一侧的消息，并复用所有公共消息字段的映射规则。
fn query_message_window_side(
    connection: &Connection,
    session_id: &SessionId,
    visibility_clause: &str,
    id_operator: &str,
    message_id: &MessageId,
    descending: bool,
    limit: u32,
) -> Result<Vec<StoredMessage>> {
    let order = if descending { "DESC" } else { "ASC" };
    let sql = format!(
        "SELECT id, session_id, role, COALESCE(content, ''), timestamp,
                tool_call_id, tool_name, tool_calls, reasoning, finish_reason,
                display_kind, display_metadata, active, compacted
         FROM messages
         WHERE session_id = ? AND {visibility_clause} AND id {id_operator} ?
         ORDER BY id {order} LIMIT ?"
    );
    let mut statement = connection.prepare(&sql).context("准备消息窗口查询失败")?;
    let rows = statement
        .query_map(
            rusqlite::params![session_id.as_str(), message_id.get(), i64::from(limit)],
            map_stored_message,
        )
        .context("执行消息窗口查询失败")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("读取消息窗口失败")
}

/// 压缩代之间判定同一展示消息的键。
type DisplayMessageKey = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn display_key(message: &StoredMessage) -> DisplayMessageKey {
    (
        message.role.clone(),
        message.content.clone(),
        message.timestamp.clone(),
        message.tool_call_id.clone(),
        message.tool_calls.clone(),
        message.tool_name.clone(),
    )
}

fn page_messages(mut messages: Vec<StoredMessage>, query: &MessageQuery) -> Vec<StoredMessage> {
    if query.latest {
        messages.reverse();
    }

    let mut page = messages.into_iter().skip(query.offset as usize);
    let result = match query.limit {
        Some(limit) => page.by_ref().take(limit as usize).collect(),
        None => page.collect(),
    };

    if query.latest {
        let mut result: Vec<_> = result;
        result.reverse();
        result
    } else {
        result
    }
}

impl Store {
    /// 读取会话消息，默认仅返回活动消息且按插入顺序排列。
    ///
    /// 消息使用 SQLite 自增 ID 排序，而不是 timestamp；系统时间回拨不会改变对话顺序。
    pub fn get_messages(
        &self,
        session_id: &SessionId,
        query: &MessageQuery,
    ) -> Result<Vec<StoredMessage>> {
        query.validate()?;

        // include_inactive 的语义是审计模式：调用方要求查看全部物理消息，
        // 因而不能再应用压缩代去重规则。
        if query.include_compacted && !query.include_inactive {
            let rows = query_messages(
                &self.connection,
                session_id,
                "(active = 1 OR compacted = 1)",
                None,
                false,
                None,
                0,
            )?;
            let mut seen: HashMap<DisplayMessageKey, StoredMessage> = HashMap::new();
            for message in rows {
                let key = display_key(&message);
                match seen.entry(key) {
                    Entry::Vacant(entry) => {
                        entry.insert(message);
                    }
                    Entry::Occupied(mut entry)
                        if (message.active, message.id.get())
                            > (entry.get().active, entry.get().id.get()) =>
                    {
                        entry.insert(message);
                    }
                    Entry::Occupied(_) => {}
                }
            }

            let mut messages: Vec<_> = seen.into_values().collect();
            messages.sort_by_key(|message| message.id.get());
            return Ok(page_messages(messages, query));
        }

        let visibility_clause = if query.include_inactive {
            "1 = 1"
        } else {
            "active = 1"
        };
        let mut messages = query_messages(
            &self.connection,
            session_id,
            visibility_clause,
            query.after_id.as_ref(),
            query.latest,
            query.limit,
            query.offset,
        )?;
        if query.latest {
            messages.reverse();
        }
        Ok(messages)
    }

    /// 读取以指定消息为锚点的前后上下文。
    ///
    /// 返回 None 表示锚点不存在、不属于指定会话，或在默认可见性规则下不可见。
    /// 每侧至多读取 window 条消息，结果仍保持按消息 ID 正序。
    pub fn get_messages_around(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        window: u32,
        include_inactive: bool,
    ) -> Result<Option<MessageWindow>> {
        let visibility_clause = if include_inactive {
            "1 = 1"
        } else {
            "active = 1"
        };
        // 第一侧包含锚点本身，因此限制为 window + 1；u32 转 i64 后仍安全。
        let mut before = query_message_window_side(
            &self.connection,
            session_id,
            visibility_clause,
            "<=",
            &message_id,
            true,
            window.saturating_add(1),
        )?;

        // 未找到锚点时，不能把另一会话或默认隐藏的消息暴露给调用方。
        if before.first().map(|message| message.id.get()) != Some(message_id.get()) {
            return Ok(None);
        }

        let after = query_message_window_side(
            &self.connection,
            session_id,
            visibility_clause,
            ">",
            &message_id,
            false,
            window,
        )?;
        before.reverse();
        let messages_before = before.len().saturating_sub(1) as u32;
        let messages_after = after.len() as u32;
        before.extend(after);

        Ok(Some(MessageWindow {
            messages: before,
            messages_before,
            messages_after,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use rusqlite::Connection;
    use sagent_types::{MessageId, SessionId};

    use super::{MessageQuery, Store};

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sagent-messages-{name}-{}.db", std::process::id()))
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
                    role TEXT NOT NULL,
                    content TEXT,
                    timestamp TEXT,
                    tool_call_id TEXT,
                    tool_name TEXT,
                    tool_calls TEXT,
                    reasoning TEXT,
                    finish_reason TEXT,
                    display_kind TEXT,
                    display_metadata TEXT,
                    active INTEGER NOT NULL,
                    compacted INTEGER NOT NULL
                 );
                 INSERT INTO messages VALUES
                    (1, 'session-1', 'user', '第一条', '2026-08-29T10:00:00Z',
                     NULL, NULL, NULL, NULL, NULL, NULL, NULL, 1, 0),
                    (2, 'session-1', 'assistant', '已撤回', '2026-08-29T10:01:00Z',
                     NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
                    (3, 'session-1', 'assistant', '工具结果', '2026-08-29T10:02:00Z',
                     'call-1', 'schema.inspect', '[{}]', '检查表结构', 'tool_calls',
                     'tool_call', '{\"collapsed\":true}', 0, 1),
                    (4, 'session-1', 'assistant', '工具结果', '2026-08-29T10:02:00Z',
                     'call-1', 'schema.inspect', '[{}]', '检查表结构', 'tool_calls',
                     'tool_call', '{\"collapsed\":true}', 1, 0),
                    (5, 'session-1', 'assistant', '最终回复', '2026-08-29T10:03:00Z',
                     NULL, NULL, NULL, NULL, 'stop', NULL, NULL, 1, 0),
                    (6, 'session-1', 'user', '仅压缩历史', '2026-08-29T10:04:00Z',
                     NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 1),
                    (7, 'other-session', 'user', '其他会话', '2026-08-29T10:05:00Z',
                     NULL, NULL, NULL, NULL, NULL, NULL, NULL, 1, 0);",
            )
            .expect("应能创建消息测试数据");
    }

    fn message_ids(messages: &[sagent_types::StoredMessage]) -> Vec<i64> {
        messages.iter().map(|message| message.id.get()).collect()
    }

    #[test]
    fn defaults_to_active_messages_in_insertion_order() {
        let path = test_path("default");
        remove(&path);
        create_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");

        let messages = store
            .get_messages(&SessionId::new("session-1"), &MessageQuery::default())
            .expect("应能读取消息");

        assert_eq!(message_ids(&messages), vec![1, 4, 5]);
        assert_eq!(messages[1].tool_name.as_deref(), Some("schema.inspect"));
        assert_eq!(messages[1].reasoning.as_deref(), Some("检查表结构"));
        remove(&path);
    }

    #[test]
    fn inactive_query_includes_soft_deleted_messages() {
        let path = test_path("inactive");
        remove(&path);
        create_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");
        let query = MessageQuery {
            include_inactive: true,
            ..MessageQuery::default()
        };

        let messages = store
            .get_messages(&SessionId::new("session-1"), &query)
            .expect("应能读取消息");

        assert_eq!(message_ids(&messages), vec![1, 2, 3, 4, 5, 6]);
        remove(&path);
    }

    #[test]
    fn compacted_query_keeps_display_history_without_duplicate_generations() {
        let path = test_path("compacted");
        remove(&path);
        create_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");
        let query = MessageQuery {
            include_compacted: true,
            ..MessageQuery::default()
        };

        let messages = store
            .get_messages(&SessionId::new("session-1"), &query)
            .expect("应能读取消息");

        assert_eq!(message_ids(&messages), vec![1, 4, 5, 6]);
        remove(&path);
    }

    #[test]
    fn inactive_query_takes_precedence_over_compacted_deduplication() {
        let path = test_path("inactive-and-compacted");
        remove(&path);
        create_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");
        let query = MessageQuery {
            include_inactive: true,
            include_compacted: true,
            ..MessageQuery::default()
        };

        let messages = store
            .get_messages(&SessionId::new("session-1"), &query)
            .expect("审计模式应返回全部消息");

        assert_eq!(message_ids(&messages), vec![1, 2, 3, 4, 5, 6]);
        remove(&path);
    }

    #[test]
    fn latest_page_is_returned_in_chronological_order() {
        let path = test_path("latest");
        remove(&path);
        create_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");
        let query = MessageQuery {
            limit: Some(1),
            offset: 1,
            latest: true,
            ..MessageQuery::default()
        };

        let messages = store
            .get_messages(&SessionId::new("session-1"), &query)
            .expect("应能读取消息");

        assert_eq!(message_ids(&messages), vec![4]);
        remove(&path);
    }

    #[test]
    fn after_id_uses_keyset_pagination_and_rejects_conflicts() {
        let path = test_path("after-id");
        remove(&path);
        create_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");
        let query = MessageQuery {
            after_id: Some(MessageId::new(1)),
            limit: Some(2),
            ..MessageQuery::default()
        };

        let messages = store
            .get_messages(&SessionId::new("session-1"), &query)
            .expect("应能读取消息");
        assert_eq!(message_ids(&messages), vec![4, 5]);

        let invalid = MessageQuery {
            after_id: Some(MessageId::new(1)),
            latest: true,
            ..MessageQuery::default()
        };
        assert!(
            store
                .get_messages(&SessionId::new("session-1"), &invalid)
                .is_err()
        );
        remove(&path);
    }

    #[test]
    fn gets_a_chronological_window_around_an_active_message() {
        let path = test_path("around-middle");
        remove(&path);
        create_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");

        let result = store
            .get_messages_around(&SessionId::new("session-1"), MessageId::new(4), 1, false)
            .expect("应能加载消息窗口")
            .expect("活动锚点应存在");

        assert_eq!(message_ids(&result.messages), vec![1, 4, 5]);
        assert_eq!(result.messages_before, 1);
        assert_eq!(result.messages_after, 1);
        remove(&path);
    }

    #[test]
    fn window_counts_expose_session_boundaries() {
        let path = test_path("around-boundary");
        remove(&path);
        create_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");

        let result = store
            .get_messages_around(&SessionId::new("session-1"), MessageId::new(1), 2, false)
            .expect("应能加载消息窗口")
            .expect("活动锚点应存在");

        assert_eq!(message_ids(&result.messages), vec![1, 4, 5]);
        assert_eq!(result.messages_before, 0);
        assert_eq!(result.messages_after, 2);
        remove(&path);
    }

    #[test]
    fn window_rejects_wrong_session_and_honors_inactive_visibility() {
        let path = test_path("around-visibility");
        remove(&path);
        create_fixture(&path);
        let store = Store::open_readonly(&path).expect("应能只读打开 fixture");

        assert!(
            store
                .get_messages_around(
                    &SessionId::new("other-session"),
                    MessageId::new(4),
                    1,
                    false,
                )
                .expect("跨会话查询本身应成功")
                .is_none()
        );
        assert!(
            store
                .get_messages_around(&SessionId::new("session-1"), MessageId::new(2), 1, false,)
                .expect("隐藏锚点查询本身应成功")
                .is_none()
        );

        let result = store
            .get_messages_around(&SessionId::new("session-1"), MessageId::new(2), 1, true)
            .expect("审计模式应能加载窗口")
            .expect("非活动锚点应在审计模式中存在");
        assert_eq!(message_ids(&result.messages), vec![1, 2, 3]);
        remove(&path);
    }
}
