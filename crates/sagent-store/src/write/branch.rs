//! Store 的会话分支回退与恢复事务。
//!
//! 回退与恢复必须比较活动消息头，避免旧分支在新输入已出现后被静默合并；所有改变
//! 都在事务中同时更新消息可见性和会话计数。

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use sagent_types::{MessageId, SessionId};

use super::{RestoreResult, RewindCheckpoint, RewindResult};
use crate::{Store, message::map_stored_message};

impl Store {
    /// 回退到一条用户消息，将目标消息本身及其后的活动消息软删除。
    ///
    /// 被回退消息仅变为 active=0，仍保留在数据库与 FTS 索引中，以便审计模式
    /// 查询或未来的恢复功能使用。目标不存在、属于其他会话、或不是用户消息时
    /// 返回错误且不会修改任何记录。
    pub fn rewind_to_message(
        &mut self,
        session_id: &SessionId,
        target_message_id: MessageId,
        updated_at: &str,
    ) -> Result<RewindResult> {
        self.ensure_writable()?;
        let transaction = self
            .connection
            .transaction()
            .context("开始消息回退事务失败")?;
        let target_message = transaction
            .query_row(
                "SELECT id, session_id, role, COALESCE(content, ''), timestamp,
                        tool_call_id, tool_name, tool_calls, reasoning, finish_reason,
                        display_kind, display_metadata, active, compacted
                 FROM messages
                 WHERE id = ?1 AND session_id = ?2",
                params![target_message_id.get(), session_id.as_str()],
                map_stored_message,
            )
            .optional()
            .context("读取回退目标消息失败")?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "消息 {} 不存在或不属于会话 {}",
                    target_message_id.get(),
                    session_id.as_str()
                )
            })?;
        if target_message.role != "user" {
            anyhow::bail!(
                "回退目标必须是 user 消息，实际角色为 {}",
                target_message.role
            );
        }

        let rewound_count = transaction
            .execute(
                "UPDATE messages
                 SET active = 0
                 WHERE session_id = ?1 AND id >= ?2 AND active = 1",
                params![session_id.as_str(), target_message_id.get()],
            )
            .context("软删除回退消息失败")? as u64;
        let active_count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1 AND active = 1",
                [session_id.as_str()],
                |row| row.get(0),
            )
            .context("重新统计活动消息失败")?;
        let new_head_id = transaction
            .query_row(
                "SELECT MAX(id) FROM messages WHERE session_id = ?1 AND active = 1",
                [session_id.as_str()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .context("读取回退后的消息头失败")?
            .map(MessageId::new);
        transaction
            .execute(
                "UPDATE sessions
                 SET message_count = ?1,
                     rewind_count = rewind_count + 1,
                     updated_at = ?2
                 WHERE id = ?3",
                params![active_count, updated_at, session_id.as_str()],
            )
            .context("更新回退后的会话状态失败")?;
        transaction.commit().context("提交消息回退事务失败")?;

        Ok(RewindResult {
            rewound_count,
            target_message,
            new_head_id: new_head_id.clone(),
            checkpoint: RewindCheckpoint {
                session_id: session_id.clone(),
                target_message_id,
                expected_active_head_id: new_head_id,
            },
        })
    }

    /// 恢复从指定消息开始被回退的非活动消息。
    ///
    /// 此接口与 rewind_to_message 构成可逆操作，主要供 TUI 的“撤销回退”使用。
    /// 它遵循 Python 的恢复语义：恢复所有 inactive 消息，不区分其是否带有
    /// compacted 标记；因此不应将它直接暴露给上下文压缩的普通流程。
    pub fn restore_rewound(
        &mut self,
        checkpoint: &RewindCheckpoint,
        updated_at: &str,
    ) -> Result<u64> {
        self.ensure_writable()?;
        let transaction = self
            .connection
            .transaction()
            .context("开始消息恢复事务失败")?;
        let session_exists = transaction
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                [checkpoint.session_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .context("验证恢复会话失败")?
            .is_some();
        if !session_exists {
            anyhow::bail!("恢复会话不存在：{}", checkpoint.session_id.as_str());
        }
        let current_head_id = transaction
            .query_row(
                "SELECT MAX(id) FROM messages WHERE session_id = ?1 AND active = 1",
                [checkpoint.session_id.as_str()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .context("读取恢复前的活动消息头失败")?
            .map(MessageId::new);
        if current_head_id != checkpoint.expected_active_head_id {
            anyhow::bail!("回退后已经出现新的活动消息，不能恢复旧分支");
        }
        let restored_count = transaction
            .execute(
                "UPDATE messages
                 SET active = 1
                 WHERE session_id = ?1 AND id >= ?2 AND active = 0",
                params![
                    checkpoint.session_id.as_str(),
                    checkpoint.target_message_id.get()
                ],
            )
            .context("恢复回退消息失败")? as u64;
        let active_count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1 AND active = 1",
                [checkpoint.session_id.as_str()],
                |row| row.get(0),
            )
            .context("重新统计恢复后的活动消息失败")?;
        transaction
            .execute(
                "UPDATE sessions
                 SET message_count = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![active_count, updated_at, checkpoint.session_id.as_str()],
            )
            .context("更新恢复后的会话状态失败")?;
        transaction.commit().context("提交消息恢复事务失败")?;
        Ok(restored_count)
    }

    /// 通过会话与回退起点恢复消息，适用于每次调用都是新进程的 CLI。
    ///
    /// 与 Python 的 `restore_rewound(session_id, since_message_id)` 一样，恢复范围是
    /// 从起点开始的 inactive 物理消息；但此版本额外拒绝新活动分支：若当前活动头已经
    /// 到达或越过回退起点，说明回退后有新消息，不能无提示地合并旧分支。
    pub fn restore_rewound_from(
        &mut self,
        session_id: &SessionId,
        target_message_id: MessageId,
        updated_at: &str,
    ) -> Result<RestoreResult> {
        self.ensure_writable()?;
        let transaction = self
            .connection
            .transaction()
            .context("开始跨进程消息恢复事务失败")?;
        let session_exists = transaction
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                [session_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .context("验证恢复会话失败")?
            .is_some();
        if !session_exists {
            anyhow::bail!("恢复会话不存在：{}", session_id.as_str());
        }
        let target_active = transaction
            .query_row(
                "SELECT active FROM messages WHERE id = ?1 AND session_id = ?2",
                params![target_message_id.get(), session_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .context("读取恢复起点失败")?
            .ok_or_else(|| anyhow::anyhow!("恢复起点不存在或不属于会话"))?;
        if target_active != 0 {
            anyhow::bail!("恢复起点仍是活动消息，无法恢复");
        }
        let current_head_id = transaction
            .query_row(
                "SELECT MAX(id) FROM messages WHERE session_id = ?1 AND active = 1",
                [session_id.as_str()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .context("读取恢复前的活动消息头失败")?;
        if current_head_id.is_some_and(|head_id| head_id >= target_message_id.get()) {
            anyhow::bail!("回退后已经出现新的活动消息，不能恢复旧分支");
        }
        let restored_count = transaction
            .execute(
                "UPDATE messages
                 SET active = 1
                 WHERE session_id = ?1 AND id >= ?2 AND active = 0",
                params![session_id.as_str(), target_message_id.get()],
            )
            .context("恢复回退消息失败")? as u64;
        let active_count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1 AND active = 1",
                [session_id.as_str()],
                |row| row.get(0),
            )
            .context("重新统计恢复后的活动消息失败")?;
        let new_head_id = transaction
            .query_row(
                "SELECT MAX(id) FROM messages WHERE session_id = ?1 AND active = 1",
                [session_id.as_str()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .context("读取恢复后的活动消息头失败")?
            .map(MessageId::new);
        transaction
            .execute(
                "UPDATE sessions
                 SET message_count = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![active_count, updated_at, session_id.as_str()],
            )
            .context("更新恢复后的会话状态失败")?;
        transaction.commit().context("提交跨进程消息恢复事务失败")?;
        Ok(RestoreResult {
            restored_count,
            new_head_id,
        })
    }
}
