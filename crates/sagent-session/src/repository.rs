//! SQLite Session Repository。
//!
//! 所有写操作使用显式事务；成功提交前不返回或更新任何成功状态。Repository 不发布事件，
//! event 由后续 Session Actor 在事务成功后发布。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 4 Repository

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sagent_types::ids::{MessageId, SessionId};
use sagent_types::message::Message;
use sagent_types::session::{Session, SessionStatus};

use crate::connection::DatabaseConnection;
use crate::error::DatabaseError;
use crate::models::{
    AppendMessage, CreateSession, ListSessions, MessageRange, SessionSnapshot, SessionSummary,
    MAX_LIST_LIMIT, MAX_MESSAGE_LIMIT,
};

static ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Session Repository 错误。
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    /// 数据库基础设施错误。
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Repository SQL 操作失败。
    #[error("Repository SQLite 操作失败: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Session 不存在。
    #[error("Session 不存在: {0:?}")]
    NotFound(SessionId),
    /// Session 已关闭，不能追加消息。
    #[error("Session 已关闭: {0:?}")]
    SessionClosed(SessionId),
    /// 输入不满足 Repository 约束。
    #[error("Repository 输入无效: {0}")]
    InvalidInput(String),
    /// 查询数量超过上限。
    #[error("查询数量超过上限: {requested}, 最大为 {maximum}")]
    LimitExceeded {
        /// 请求数量。
        requested: u32,
        /// 最大数量。
        maximum: u32,
    },
    /// 数据库中持久化的 JSON 或 enum 损坏。
    #[error("数据库记录损坏: {0}")]
    CorruptData(String),
    /// Session 中消息数和实际消息不一致。
    #[error("Session transcript 不一致: {0}")]
    InconsistentSnapshot(String),
}

/// SQLite Session Repository。
pub struct Repository {
    database: DatabaseConnection,
}

impl std::fmt::Debug for Repository {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Repository")
            .field("database_path", &self.database.path())
            .field("schema_version", &self.database.schema_version())
            .finish()
    }
}

impl Repository {
    /// 从已完成 migration 的数据库连接创建 Repository。
    pub fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// 返回数据库文件路径，不暴露数据库连接。
    pub fn database_path(&self) -> &std::path::Path {
        self.database.path()
    }

    /// 创建并持久化一个 Session。
    pub fn create_session(&mut self, input: CreateSession) -> Result<Session, RepositoryError> {
        validate_metadata(&input.metadata, "metadata")?;
        if input.source.trim().is_empty() {
            return Err(RepositoryError::InvalidInput("source 不能为空".to_string()));
        }
        let id = SessionId(next_id("sess"));
        let now = now_rfc3339();
        let metadata = serde_json::to_string(&input.metadata)
            .map_err(|error| RepositoryError::CorruptData(error.to_string()))?;
        let transaction = self
            .database
            .connection_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO sessions(\
                id, source, title, cwd, status, metadata_json, created_at, updated_at,\
                message_count, revision\
             ) VALUES(?1, ?2, ?3, ?4, 'active', ?5, ?6, ?6, 0, 0)",
            params![id.0, input.source, input.title, input.cwd, metadata, now],
        )?;
        transaction.commit()?;
        self.get_session(&id)?.ok_or(RepositoryError::NotFound(id))
    }

    /// 获取一个 Session；不存在时返回 `None`。
    pub fn get_session(&self, session_id: &SessionId) -> Result<Option<Session>, RepositoryError> {
        let connection = self.database_connection();
        let raw = connection
            .query_row(
                "SELECT id, source, title, cwd, status, metadata_json, created_at, updated_at, \
                 message_count, revision FROM sessions WHERE id = ?1",
                [session_id.0.as_str()],
                raw_session_from_row,
            )
            .optional()?;
        raw.map(decode_session).transpose()
    }

    /// 按 updated_at DESC、id ASC 稳定列出 Session。
    pub fn list_sessions(
        &self,
        query: ListSessions,
    ) -> Result<Vec<SessionSummary>, RepositoryError> {
        let limit = checked_limit(query.limit, MAX_LIST_LIMIT)?;
        let connection = self.database_connection();
        let mut statement = connection.prepare(
            "SELECT id, source, title, cwd, status, metadata_json, created_at, updated_at, \
             message_count, revision FROM sessions \
             WHERE (?1 IS NULL OR source = ?1) \
               AND (?2 IS NULL OR status = ?2) \
               AND (?3 IS NULL OR updated_at < ?3 OR (updated_at = ?3 AND id > ?4)) \
             ORDER BY updated_at DESC, id ASC LIMIT ?5",
        )?;
        let source = query.source;
        let status = query.status.map(status_string);
        let cursor_updated = query.before.as_ref().map(|cursor| cursor.updated_at.clone());
        let cursor_id = query.before.as_ref().map(|cursor| cursor.id.0.clone());
        let rows = statement.query_map(
            params![source, status, cursor_updated, cursor_id, i64::from(limit)],
            raw_session_from_row,
        )?;
        rows.map(|row| {
            let session = decode_session(row?)?;
            Ok(session.into())
        })
        .collect()
    }

    /// 在事务中追加消息、更新计数和 revision。
    pub fn append_message(
        &mut self,
        session_id: &SessionId,
        input: AppendMessage,
    ) -> Result<Message, RepositoryError> {
        validate_metadata(&input.metadata, "metadata")?;
        if input.content.is_empty() {
            return Err(RepositoryError::InvalidInput(
                "content 不能为空".to_string(),
            ));
        }
        let created_at = now_rfc3339();
        let message_id = MessageId(next_id("msg"));
        let content = serde_json::to_string(&input.content)
            .map_err(|error| RepositoryError::CorruptData(error.to_string()))?;
        let tool_calls = serde_json::to_string(&input.tool_calls)
            .map_err(|error| RepositoryError::CorruptData(error.to_string()))?;
        let metadata = serde_json::to_string(&input.metadata)
            .map_err(|error| RepositoryError::CorruptData(error.to_string()))?;
        let role = serde_json::to_value(&input.role)
            .map_err(|error| RepositoryError::CorruptData(error.to_string()))?
            .as_str()
            .ok_or_else(|| RepositoryError::CorruptData("role 不是字符串".to_string()))?
            .to_string();
        let tool_call_id = input.tool_call_id.as_ref().map(|id| id.0.clone());
        let transaction = self
            .database
            .connection_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let session_state: Option<(String, i64, i64)> = transaction
            .query_row(
                "SELECT status, message_count, revision FROM sessions WHERE id = ?1",
                [session_id.0.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((status, message_count, revision)) = session_state else {
            return Err(RepositoryError::NotFound(session_id.clone()));
        };
        if status != "active" {
            return Err(RepositoryError::SessionClosed(session_id.clone()));
        }
        let sequence: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM messages WHERE session_id = ?1",
            [session_id.0.as_str()],
            |row| row.get(0),
        )?;
        if message_count < 0 || revision < 0 || sequence <= 0 {
            return Err(RepositoryError::CorruptData(
                "Session 计数或 sequence 为负数".to_string(),
            ));
        }
        transaction.execute(
            "INSERT INTO messages(\
                id, session_id, sequence, role, content_json, tool_calls_json,\
                tool_call_id, metadata_json, created_at\
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                message_id.0,
                session_id.0,
                sequence,
                role,
                content,
                tool_calls,
                tool_call_id,
                metadata,
                created_at,
            ],
        )?;
        let updated_at = now_rfc3339();
        let changed = transaction.execute(
            "UPDATE sessions SET message_count = message_count + 1,\
             updated_at = ?1, revision = revision + 1 WHERE id = ?2 AND status = 'active'",
            params![updated_at, session_id.0.as_str()],
        )?;
        if changed != 1 {
            return Err(RepositoryError::CorruptData(
                "Session 更新计数失败".to_string(),
            ));
        }
        transaction.commit()?;
        Ok(Message {
            message_id,
            session_id: session_id.clone(),
            role: input.role,
            content: input.content,
            tool_calls: input.tool_calls,
            tool_call_id: input.tool_call_id,
            created_at,
            sequence: sequence as u64,
            metadata: input.metadata,
        })
    }

    /// 按 sequence 升序读取受限消息窗口。
    pub fn get_messages(
        &self,
        session_id: &SessionId,
        range: MessageRange,
    ) -> Result<Vec<Message>, RepositoryError> {
        if self.get_session(session_id)?.is_none() {
            return Err(RepositoryError::NotFound(session_id.clone()));
        }
        let limit = checked_limit(range.limit, MAX_MESSAGE_LIMIT)?;
        let connection = self.database_connection();
        let mut statement = connection.prepare(
            "SELECT id, session_id, sequence, role, content_json, tool_calls_json, \
             tool_call_id, metadata_json, created_at FROM messages \
             WHERE session_id = ?1 AND (?2 IS NULL OR sequence > ?2) \
             ORDER BY sequence ASC LIMIT ?3",
        )?;
        let after = range.after_sequence.map(|value| value as i64);
        let rows = statement.query_map(
            params![session_id.0.as_str(), after, i64::from(limit)],
            raw_message_from_row,
        )?;
        rows.map(|row| decode_message(row?)).collect()
    }

    /// 恢复 Session 和完整但受上限保护的消息窗口。
    pub fn resume_session(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionSnapshot, RepositoryError> {
        let session = self
            .get_session(session_id)?
            .ok_or(RepositoryError::NotFound(session_id.clone()))?;
        if session.message_count > u64::from(MAX_MESSAGE_LIMIT) {
            return Err(RepositoryError::LimitExceeded {
                requested: session.message_count as u32,
                maximum: MAX_MESSAGE_LIMIT,
            });
        }
        let messages = self.get_messages(
            session_id,
            MessageRange {
                limit: Some(MAX_MESSAGE_LIMIT),
                after_sequence: None,
            },
        )?;
        if messages.len() as u64 != session.message_count {
            return Err(RepositoryError::InconsistentSnapshot(format!(
                "message_count={}，实际={}",
                session.message_count,
                messages.len()
            )));
        }
        if let Some(last) = messages.last() {
            if last.sequence != session.message_count {
                return Err(RepositoryError::InconsistentSnapshot(format!(
                    "最后 sequence={}，count={}",
                    last.sequence, session.message_count
                )));
            }
        }
        Ok(SessionSnapshot { session, messages })
    }

    /// 关闭 Session；重复关闭返回当前状态，不重复增加 revision。
    pub fn close_session(
        &mut self,
        session_id: &SessionId,
        _reason: Option<&str>,
    ) -> Result<Session, RepositoryError> {
        let transaction = self
            .database
            .connection_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let status: Option<String> = transaction
            .query_row(
                "SELECT status FROM sessions WHERE id = ?1",
                [session_id.0.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(status) = status else {
            return Err(RepositoryError::NotFound(session_id.clone()));
        };
        if status == "active" {
            transaction.execute(
                "UPDATE sessions SET status = 'closed', updated_at = ?1, revision = revision + 1 \
                 WHERE id = ?2",
                params![now_rfc3339(), session_id.0.as_str()],
            )?;
        }
        transaction.commit()?;
        self.get_session(session_id)?
            .ok_or(RepositoryError::NotFound(session_id.clone()))
    }

    fn database_connection(&self) -> &Connection {
        // Repository owns the connection and only uses this private read view internally.
        self.database.connection_ref()
    }
}

fn checked_limit(limit: Option<u32>, maximum: u32) -> Result<u32, RepositoryError> {
    let limit = limit.unwrap_or(maximum.min(50));
    if limit == 0 {
        return Err(RepositoryError::InvalidInput(
            "limit 必须大于 0".to_string(),
        ));
    }
    if limit > maximum {
        return Err(RepositoryError::LimitExceeded {
            requested: limit,
            maximum,
        });
    }
    Ok(limit)
}

fn validate_metadata(
    metadata: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<(), RepositoryError> {
    if metadata.len() > 128 {
        return Err(RepositoryError::InvalidInput(format!(
            "{field} 字段数量过多"
        )));
    }
    Ok(())
}

fn raw_session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawSession> {
    Ok(RawSession {
        id: row.get(0)?,
        source: row.get(1)?,
        title: row.get(2)?,
        cwd: row.get(3)?,
        status: row.get(4)?,
        metadata_json: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        message_count: row.get(8)?,
        revision: row.get(9)?,
    })
}

fn decode_session(raw: RawSession) -> Result<Session, RepositoryError> {
    let metadata = serde_json::from_str(&raw.metadata_json)
        .map_err(|error| RepositoryError::CorruptData(format!("session metadata: {error}")))?;
    let status = parse_status(&raw.status)?;
    if raw.message_count < 0 || raw.revision < 0 {
        return Err(RepositoryError::CorruptData(
            "Session count/revision 为负数".to_string(),
        ));
    }
    Ok(Session {
        id: SessionId(raw.id),
        source: raw.source,
        title: raw.title,
        created_at: raw.created_at,
        updated_at: raw.updated_at,
        status,
        cwd: raw.cwd,
        metadata,
        message_count: raw.message_count as u64,
        revision: raw.revision as u64,
    })
}

fn raw_message_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawMessage> {
    Ok(RawMessage {
        id: row.get(0)?,
        session_id: row.get(1)?,
        sequence: row.get(2)?,
        role: row.get(3)?,
        content_json: row.get(4)?,
        tool_calls_json: row.get(5)?,
        tool_call_id: row.get(6)?,
        metadata_json: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn decode_message(raw: RawMessage) -> Result<Message, RepositoryError> {
    if raw.sequence <= 0 {
        return Err(RepositoryError::CorruptData(
            "Message sequence 无效".to_string(),
        ));
    }
    let role = parse_role(&raw.role)?;
    let content = serde_json::from_str(&raw.content_json)
        .map_err(|error| RepositoryError::CorruptData(format!("message content: {error}")))?;
    let tool_calls = serde_json::from_str(&raw.tool_calls_json)
        .map_err(|error| RepositoryError::CorruptData(format!("message tool_calls: {error}")))?;
    let metadata = serde_json::from_str(&raw.metadata_json)
        .map_err(|error| RepositoryError::CorruptData(format!("message metadata: {error}")))?;
    Ok(Message {
        message_id: MessageId(raw.id),
        session_id: SessionId(raw.session_id),
        role,
        content,
        tool_calls,
        tool_call_id: raw.tool_call_id.map(sagent_types::ids::ToolCallId),
        created_at: raw.created_at,
        sequence: raw.sequence as u64,
        metadata,
    })
}

fn parse_status(value: &str) -> Result<SessionStatus, RepositoryError> {
    serde_json::from_value(serde_json::Value::String(value.to_string()))
        .map_err(|error| RepositoryError::CorruptData(format!("session status: {error}")))
}

fn parse_role(value: &str) -> Result<sagent_types::message::Role, RepositoryError> {
    serde_json::from_value(serde_json::Value::String(value.to_string()))
        .map_err(|error| RepositoryError::CorruptData(format!("message role: {error}")))
}

fn status_string(status: SessionStatus) -> String {
    match status {
        SessionStatus::Active => "active".to_string(),
        SessionStatus::Closed => "closed".to_string(),
        SessionStatus::Recovering => "recovering".to_string(),
    }
}

struct RawSession {
    id: String,
    source: String,
    title: Option<String>,
    cwd: Option<String>,
    status: String,
    metadata_json: String,
    created_at: String,
    updated_at: String,
    message_count: i64,
    revision: i64,
}

struct RawMessage {
    id: String,
    session_id: String,
    sequence: i64,
    role: String,
    content_json: String,
    tool_calls_json: String,
    tool_call_id: Option<String>,
    metadata_json: String,
    created_at: String,
}

fn next_id(prefix: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let sequence = ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{millis}_{sequence}")
}

fn now_rfc3339() -> String {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = duration.as_secs();
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds_of_day / 3_600,
        (seconds_of_day / 60) % 60,
        seconds_of_day % 60,
        duration.subsec_millis()
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}
