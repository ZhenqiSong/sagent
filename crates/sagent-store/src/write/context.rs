//! Store 的上下文替换、压缩与重试事务。
//!
//! 这些操作都在单个 SQLite transaction 中重写活动分支；拆到子模块仅为按生命周期
//! 组织代码，仍通过 Store 的私有连接执行，不能绕开写保护或事务边界。

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use sagent_types::{MessageId, SessionId};

use super::{NewMessage, RetryCheckpoint, insert_message};
use crate::Store;

impl Store {
    /// 软归档当前活动消息，并原子写入一组新的活动消息。
    ///
    /// replacements 必须全部属于 session_id。空替换集是合法操作，表示清空活动
    /// 上下文但仍保留旧消息供审计；不会删除物理消息或其 FTS 索引条目。
    pub fn replace_active_messages(
        &mut self,
        session_id: &SessionId,
        replacements: &[NewMessage],
        updated_at: &str,
    ) -> Result<Vec<MessageId>> {
        self.ensure_writable()?;
        if replacements
            .iter()
            .any(|message| message.session_id != *session_id)
        {
            anyhow::bail!("替换消息中存在不属于目标会话的记录");
        }

        let transaction = self
            .connection
            .transaction()
            .context("开始消息替换事务失败")?;
        let session_exists = transaction
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                [session_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .context("验证替换会话失败")?
            .is_some();
        if !session_exists {
            anyhow::bail!("替换会话不存在：{}", session_id.as_str());
        }
        transaction
            .execute(
                "UPDATE messages
                 SET active = 0, compacted = 0
                 WHERE session_id = ?1 AND active = 1",
                [session_id.as_str()],
            )
            .context("软归档原活动消息失败")?;

        let mut inserted_ids = Vec::with_capacity(replacements.len());
        for message in replacements {
            inserted_ids.push(insert_message(&transaction, message)?);
        }
        let latest_timestamp = replacements
            .last()
            .map(|message| message.timestamp.as_str());
        transaction
            .execute(
                "UPDATE sessions
                 SET message_count = ?1,
                     last_activity_at = COALESCE(?2, last_activity_at),
                     updated_at = ?3
                 WHERE id = ?4",
                params![
                    replacements.len() as i64,
                    latest_timestamp,
                    updated_at,
                    session_id.as_str()
                ],
            )
            .context("更新替换后的会话状态失败")?;
        transaction.commit().context("提交消息替换事务失败")?;
        Ok(inserted_ids)
    }

    /// 将当前活动上下文归档为压缩历史，并写入新的压缩后活动消息。
    ///
    /// 与 replace_active_messages 不同，旧消息会标记为 compacted=1，因此默认
    /// 全文搜索仍可检索到历史知识；默认消息读取则只恢复新的活动摘要。
    pub fn archive_and_compact(
        &mut self,
        session_id: &SessionId,
        compacted_messages: &[NewMessage],
        updated_at: &str,
    ) -> Result<Vec<MessageId>> {
        self.ensure_writable()?;
        if compacted_messages.is_empty() {
            anyhow::bail!("压缩结果不能为空");
        }
        if compacted_messages
            .iter()
            .any(|message| message.session_id != *session_id)
        {
            anyhow::bail!("压缩消息中存在不属于目标会话的记录");
        }

        let transaction = self
            .connection
            .transaction()
            .context("开始上下文压缩事务失败")?;
        let session_exists = transaction
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                [session_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .context("验证压缩会话失败")?
            .is_some();
        if !session_exists {
            anyhow::bail!("压缩会话不存在：{}", session_id.as_str());
        }
        transaction
            .execute(
                "UPDATE messages
                 SET active = 0, compacted = 1
                 WHERE session_id = ?1 AND active = 1",
                [session_id.as_str()],
            )
            .context("归档压缩前活动消息失败")?;

        let mut inserted_ids = Vec::with_capacity(compacted_messages.len());
        for message in compacted_messages {
            inserted_ids.push(insert_message(&transaction, message)?);
        }
        let latest_timestamp = compacted_messages
            .last()
            .map(|message| message.timestamp.as_str());
        transaction
            .execute(
                "UPDATE sessions
                 SET message_count = ?1,
                     last_activity_at = ?2,
                     updated_at = ?3
                 WHERE id = ?4",
                params![
                    compacted_messages.len() as i64,
                    latest_timestamp,
                    updated_at,
                    session_id.as_str()
                ],
            )
            .context("更新压缩后的会话状态失败")?;
        transaction.commit().context("提交上下文压缩事务失败")?;
        Ok(inserted_ids)
    }

    /// 为最新活动 assistant 消息创建重试检查点。
    ///
    /// 第一版只支持最新回答的重试，避免中间回答重试导致后续分支含义不明确。
    /// 实际模型调用应发生在本方法与 apply_retry 之间的事务外。
    pub fn prepare_retry(
        &self,
        session_id: &SessionId,
        assistant_message_id: MessageId,
    ) -> Result<RetryCheckpoint> {
        let target = self
            .connection
            .query_row(
                "SELECT role, active FROM messages
                 WHERE id = ?1 AND session_id = ?2",
                params![assistant_message_id.get(), session_id.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0)),
            )
            .optional()
            .context("读取重试目标消息失败")?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "消息 {} 不存在或不属于会话 {}",
                    assistant_message_id.get(),
                    session_id.as_str()
                )
            })?;
        if target.0 != "assistant" {
            anyhow::bail!("重试目标必须是 assistant 消息，实际角色为 {}", target.0);
        }
        if !target.1 {
            anyhow::bail!("重试目标不是活动消息");
        }
        let active_head_id = self
            .connection
            .query_row(
                "SELECT MAX(id) FROM messages WHERE session_id = ?1 AND active = 1",
                [session_id.as_str()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .context("读取活动消息头失败")?
            .map(MessageId::new)
            .ok_or_else(|| anyhow::anyhow!("会话不存在活动消息"))?;
        if active_head_id != assistant_message_id {
            anyhow::bail!("第一版仅支持重试最新 assistant 消息");
        }

        Ok(RetryCheckpoint {
            session_id: session_id.clone(),
            target_message_id: assistant_message_id,
            expected_active_head_id: active_head_id,
        })
    }

    /// 用新 assistant 消息替换检查点指向的最新活动回答。
    ///
    /// 调用前可在事务外执行模型请求；提交时重新检查活动消息头，以保证新的用户
    /// 输入或其他窗口写入不会被覆盖。
    pub fn apply_retry(
        &mut self,
        checkpoint: &RetryCheckpoint,
        replacement: &NewMessage,
        updated_at: &str,
    ) -> Result<MessageId> {
        self.ensure_writable()?;
        if replacement.session_id != checkpoint.session_id {
            anyhow::bail!("替换消息不属于重试会话");
        }
        if replacement.role != "assistant" {
            anyhow::bail!("重试替换消息必须是 assistant 角色");
        }

        let transaction = self
            .connection
            .transaction()
            .context("开始消息重试事务失败")?;
        let current_head_id = transaction
            .query_row(
                "SELECT MAX(id) FROM messages WHERE session_id = ?1 AND active = 1",
                [checkpoint.session_id.as_str()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .context("读取重试前活动消息头失败")?
            .map(MessageId::new);
        if current_head_id.as_ref() != Some(&checkpoint.expected_active_head_id) {
            anyhow::bail!("重试期间出现新的活动消息，不能覆盖当前分支");
        }
        let target_is_still_active: Option<i64> = transaction
            .query_row(
                "SELECT 1 FROM messages
                 WHERE id = ?1 AND session_id = ?2
                   AND role = 'assistant' AND active = 1",
                params![
                    checkpoint.target_message_id.get(),
                    checkpoint.session_id.as_str()
                ],
                |row| row.get(0),
            )
            .optional()
            .context("验证重试目标失败")?;
        if target_is_still_active.is_none() {
            anyhow::bail!("重试目标已不再是活动 assistant 消息");
        }
        transaction
            .execute(
                "UPDATE messages
                 SET active = 0, compacted = 0
                 WHERE id = ?1 AND session_id = ?2 AND active = 1",
                params![
                    checkpoint.target_message_id.get(),
                    checkpoint.session_id.as_str()
                ],
            )
            .context("软归档旧 assistant 回答失败")?;
        let replacement_id = insert_message(&transaction, replacement)?;
        let active_count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1 AND active = 1",
                [checkpoint.session_id.as_str()],
                |row| row.get(0),
            )
            .context("重新统计重试后的活动消息失败")?;
        transaction
            .execute(
                "UPDATE sessions
                 SET message_count = ?1, last_activity_at = ?2, updated_at = ?3
                 WHERE id = ?4",
                params![
                    active_count,
                    replacement.timestamp,
                    updated_at,
                    checkpoint.session_id.as_str()
                ],
            )
            .context("更新重试后的会话状态失败")?;
        transaction.commit().context("提交消息重试事务失败")?;
        Ok(replacement_id)
    }
}
