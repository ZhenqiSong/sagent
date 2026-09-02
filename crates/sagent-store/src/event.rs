//! daemon event 的事务内写入辅助函数。

use anyhow::{Context, Result};
use rusqlite::{Transaction, params};
use sagent_types::{EventSequence, SessionId, TurnId};

pub const EVENT_TURN_STARTED: &str = "turn.started";
pub const EVENT_MESSAGE_COMMITTED: &str = "message.committed";

#[derive(Clone, Debug)]
pub struct NewDaemonEvent {
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
