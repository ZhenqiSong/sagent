//! Turn generation 与开始回合的原子持久化。

use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use sagent_types::{MessageId, SessionId, TurnId};

use crate::{
    Store,
    event::{EVENT_MESSAGE_COMMITTED, EVENT_TURN_STARTED, NewDaemonEvent, insert_event},
    write::{NewMessage, insert_message},
};

#[derive(Clone, Debug)]
pub struct NewGeneration {
    pub session_id: SessionId,
    pub generation: i64,
    pub system_hash: String,
    pub tool_schema_hash: String,
    pub model_id: String,
    pub profile_revision: String,
    pub created_at: String,
}

#[derive(Clone, Debug)]
pub struct StartTurn {
    pub turn_id: TurnId,
    pub session_id: SessionId,
    pub generation: i64,
    pub started_at: String,
}

impl Store {
    pub fn create_generation(&mut self, generation: &NewGeneration) -> Result<()> {
        self.ensure_writable()?;
        if generation.generation < 0 {
            bail!("generation 不能为负数");
        }
        if generation.system_hash.is_empty() || generation.tool_schema_hash.is_empty() {
            bail!("generation hash 不能为空");
        }
        let transaction = self
            .connection
            .transaction()
            .context("开始 generation 创建事务失败")?;
        let exists: Option<i64> = transaction
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                [generation.session_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .context("验证 generation 所属会话失败")?;
        if exists.is_none() {
            bail!("会话不存在：{}", generation.session_id.as_str());
        }
        transaction
            .execute(
                "INSERT INTO session_generations (session_id, generation, system_hash, tool_schema_hash, model_id, profile_revision, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![generation.session_id.as_str(), generation.generation, generation.system_hash, generation.tool_schema_hash, generation.model_id, generation.profile_revision, generation.created_at],
            )
            .context("写入 session generation 失败")?;
        transaction.commit().context("提交 generation 创建事务失败")
    }

    pub fn begin_turn(&mut self, turn: &StartTurn, user_message: &NewMessage) -> Result<MessageId> {
        self.ensure_writable()?;
        if turn.generation < 0 {
            bail!("generation 不能为负数");
        }
        if user_message.session_id != turn.session_id {
            bail!("用户消息与 Turn 不属于同一会话");
        }
        if user_message.role != "user" {
            bail!("begin_turn 的消息角色必须是 user");
        }
        let transaction = self
            .connection
            .transaction()
            .context("开始 Turn 创建事务失败")?;
        let session_exists: Option<i64> = transaction
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                [turn.session_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .context("验证 Turn 所属会话失败")?;
        if session_exists.is_none() {
            bail!("会话不存在：{}", turn.session_id.as_str());
        }
        let generation_exists: Option<i64> = transaction
            .query_row(
                "SELECT 1 FROM session_generations WHERE session_id = ?1 AND generation = ?2",
                params![turn.session_id.as_str(), turn.generation],
                |row| row.get(0),
            )
            .optional()
            .context("验证 Turn generation 失败")?;
        if generation_exists.is_none() {
            bail!(
                "generation 不存在：{} / {}",
                turn.session_id.as_str(),
                turn.generation
            );
        }
        let turn_exists: Option<i64> = transaction
            .query_row(
                "SELECT 1 FROM turns WHERE turn_id = ?1",
                [turn.turn_id.as_uuid().to_string()],
                |row| row.get(0),
            )
            .optional()
            .context("验证 Turn 是否重复失败")?;
        if turn_exists.is_some() {
            bail!("Turn 已存在：{}", turn.turn_id.as_uuid());
        }
        let message_id = insert_message(&transaction, user_message)?;
        let changed = transaction.execute("UPDATE sessions SET message_count = message_count + 1, last_activity_at = ?1, updated_at = ?1 WHERE id = ?2", params![user_message.timestamp, turn.session_id.as_str()]).context("更新会话消息计数失败")?;
        if changed != 1 {
            bail!("消息所属会话不存在：{}", turn.session_id.as_str());
        }
        transaction.execute("INSERT INTO turns (turn_id, session_id, generation, status, user_message_id, started_at) VALUES (?1, ?2, ?3, 'running', ?4, ?5)", params![turn.turn_id.as_uuid().to_string(), turn.session_id.as_str(), turn.generation, message_id.get(), turn.started_at]).context("写入 running Turn 失败")?;
        insert_event(
            &transaction,
            &NewDaemonEvent {
                session_id: turn.session_id.clone(),
                turn_id: Some(turn.turn_id),
                event_type: EVENT_TURN_STARTED.to_owned(),
                payload: serde_json::json!({"turn_id": turn.turn_id, "generation": turn.generation, "user_message_id": message_id}),
                created_at: turn.started_at.clone(),
            },
        )?;
        insert_event(
            &transaction,
            &NewDaemonEvent {
                session_id: turn.session_id.clone(),
                turn_id: Some(turn.turn_id),
                event_type: EVENT_MESSAGE_COMMITTED.to_owned(),
                payload: serde_json::json!({"message_id": message_id, "role": "user"}),
                created_at: user_message.timestamp.clone(),
            },
        )?;
        transaction.commit().context("提交 Turn 创建事务失败")?;
        Ok(message_id)
    }
}
