//! daemon event 的事务内写入辅助函数。

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, Transaction, params};
use sagent_types::{EventSequence, SessionId, TurnId};

use crate::Store;

pub const EVENT_TURN_STARTED: &str = "turn.started";
pub const EVENT_MESSAGE_COMMITTED: &str = "message.committed";
pub const EVENT_TOOL_COMPLETED: &str = "tool.completed";
pub const EVENT_TURN_COMPLETED: &str = "turn.completed";
pub const EVENT_TURN_INTERRUPTED: &str = "turn.interrupted";
pub const EVENT_TURN_FAILED: &str = "turn.failed";
pub const MAX_EVENT_LIMIT: i64 = 200;

#[derive(Clone, Debug)]
pub struct NewDaemonEvent {
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_at: String,
}

#[derive(Clone, Debug)]
pub struct EventQuery {
    pub session_id: SessionId,
    pub after_sequence: EventSequence,
    pub limit: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredDaemonEvent {
    pub sequence: EventSequence,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_at: String,
}

pub(crate) fn insert_event(
    transaction: &Transaction<'_>,
    event: &NewDaemonEvent,
) -> Result<EventSequence> {
    let payload_json =
        serde_json::to_string(&event.payload).context("序列化 daemon event payload 失败")?;
    transaction
        .execute(
            "INSERT INTO daemon_events (session_id, turn_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.session_id.as_str(),
                event.turn_id.map(|id| id.as_uuid().to_string()),
                event.event_type,
                payload_json,
                event.created_at,
            ],
        )
        .context("写入 daemon event 失败")?;
    EventSequence::new(transaction.last_insert_rowid()).context("解析 daemon event sequence 失败")
}

impl Store {
    /// 按 session 和单调 sequence 查询可恢复的持久化事件。
    pub fn events_since(&self, query: &EventQuery) -> Result<Vec<StoredDaemonEvent>> {
        if query.limit <= 0 {
            anyhow::bail!("事件查询 limit 必须大于 0");
        }
        let limit = query.limit.min(MAX_EVENT_LIMIT);
        let exists: Option<i64> = self
            .connection
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                [query.session_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .context("验证事件查询所属会话失败")?;
        if exists.is_none() {
            anyhow::bail!("会话不存在：{}", query.session_id.as_str());
        }
        let mut statement = self
            .connection
            .prepare(
                "SELECT sequence, session_id, turn_id, event_type, payload_json, created_at
             FROM daemon_events WHERE session_id = ?1 AND sequence > ?2
             ORDER BY sequence ASC LIMIT ?3",
            )
            .context("准备事件查询失败")?;
        let rows = statement
            .query_map(
                params![query.session_id.as_str(), query.after_sequence.get(), limit],
                |row| {
                    let sequence: i64 = row.get(0)?;
                    let turn_id_text: Option<String> = row.get(2)?;
                    let payload_json: String = row.get(4)?;
                    let turn_id = turn_id_text
                        .map(|value| {
                            TurnId::parse(&value).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    2,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })
                        })
                        .transpose()?;
                    let payload = serde_json::from_str(&payload_json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    Ok(StoredDaemonEvent {
                        sequence: EventSequence::new(sequence).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Integer,
                                Box::new(error),
                            )
                        })?,
                        session_id: SessionId::new(row.get::<_, String>(1)?),
                        turn_id,
                        event_type: row.get(3)?,
                        payload,
                        created_at: row.get(5)?,
                    })
                },
            )
            .context("执行事件查询失败")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("读取事件结果失败")
    }

    pub fn latest_event_sequence(&self, session_id: &SessionId) -> Result<Option<EventSequence>> {
        let value: Option<i64> = self
            .connection
            .query_row(
                "SELECT MAX(sequence) FROM daemon_events WHERE session_id = ?1",
                [session_id.as_str()],
                |row| row.get(0),
            )
            .context("查询最新事件序号失败")?;
        value
            .map(|sequence| EventSequence::new(sequence).context("解析最新事件序号失败"))
            .transpose()
    }
}
