//! 会话和消息的最小写入接口。
//!
//! 作者：SongZQ

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, Transaction, params};
use sagent_types::{MessageId, SessionId, StoredMessage};

use crate::{Store, message::map_stored_message};

/// 不应被聊天记录界面渲染、但仍会进入模型上下文的消息类型。
pub const HIDDEN_DISPLAY_KIND: &str = "hidden";

/// 创建会话所需的稳定元数据。
#[derive(Clone, Debug)]
pub struct NewSession {
    /// 调用方生成的会话标识。
    pub id: SessionId,
    /// 会话来源，例如 cli、tui 或 gateway。
    pub source: Option<String>,
    /// 创建会话时选定的模型名称。
    pub model: Option<String>,
    /// 可选的用户标题。
    pub title: Option<String>,
    /// RFC 3339 格式的创建时间，由上层时钟提供。
    pub started_at: String,
}

/// 追加一条消息所需的数据。
#[derive(Clone, Debug)]
pub struct NewMessage {
    /// 目标会话。
    pub session_id: SessionId,
    /// OpenAI 兼容角色，例如 user、assistant 或 tool。
    pub role: String,
    /// 原始消息文本。
    pub content: String,
    /// RFC 3339 格式的消息时间，由上层时钟提供。
    pub timestamp: String,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_calls: Option<String>,
    pub reasoning: Option<String>,
    pub finish_reason: Option<String>,
    pub display_kind: Option<String>,
    pub display_metadata: Option<String>,
}

/// 一次消息回退的结果。
#[derive(Clone, Debug)]
pub struct RewindResult {
    /// 本次从活动状态变为非活动状态的消息数量。
    pub rewound_count: u64,
    /// 被选中的用户消息；TUI 可将其文本重新填入输入框。
    pub target_message: StoredMessage,
    /// 回退后最后一条仍活动的消息；没有活动消息时为 None。
    pub new_head_id: Option<MessageId>,
    /// 仅在活动分支未变化时才允许恢复的检查点。
    pub checkpoint: RewindCheckpoint,
}

/// 回退操作生成的恢复许可。
///
/// 调用 restore_rewound 时必须原样传回此值，以避免将旧分支混入回退后
/// 新生成的活动分支。
#[derive(Clone, Debug)]
pub struct RewindCheckpoint {
    /// 回退发生的会话。
    pub session_id: SessionId,
    /// 被回退的第一个消息 ID，也是恢复的起点。
    pub target_message_id: MessageId,
    /// 回退完成时的活动消息头；恢复前必须仍与数据库一致。
    pub expected_active_head_id: Option<MessageId>,
}

/// 在模型调用前确认的最新 assistant 消息重试许可。
#[derive(Clone, Debug)]
pub struct RetryCheckpoint {
    /// 重试所在会话。
    pub session_id: SessionId,
    /// 即将被新回答替换的 assistant 消息。
    pub target_message_id: MessageId,
    /// 创建许可时的活动消息头，用于发现并发追加的新分支。
    pub expected_active_head_id: MessageId,
}

impl NewMessage {
    /// 用消息的必要字段创建记录，其余展示与工具元数据默认为空。
    pub fn new(
        session_id: SessionId,
        role: impl Into<String>,
        content: impl Into<String>,
        timestamp: impl Into<String>,
    ) -> Self {
        Self {
            session_id,
            role: role.into(),
            content: content.into(),
            timestamp: timestamp.into(),
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
            reasoning: None,
            finish_reason: None,
            display_kind: None,
            display_metadata: None,
        }
    }

    /// 创建上下文压缩生成的隐藏摘要。
    ///
    /// 摘要仍是活动消息，因此会在会话恢复时提供给模型；但展示层应依据
    /// \`display_kind = "hidden"\` 将它排除，改为展示被归档的原始消息。
    /// role 由压缩器按相邻消息的角色交替规则决定，通常为 assistant，
    /// 必要时可以为 user，不能在存储层被固定为 system。
    pub fn compressed_summary(
        session_id: SessionId,
        role: impl Into<String>,
        content: impl Into<String>,
        timestamp: impl Into<String>,
    ) -> Self {
        let mut message = Self::new(session_id, role, content, timestamp);
        message.display_kind = Some(HIDDEN_DISPLAY_KIND.to_owned());
        message.display_metadata = Some(r#"{"compressed_summary":true}"#.to_owned());
        message
    }
}

/// 在调用方已创建的事务中写入一条消息，供追加与整段替换复用。
fn insert_message(transaction: &Transaction<'_>, message: &NewMessage) -> Result<MessageId> {
    transaction
        .execute(
            "INSERT INTO messages (
                session_id, role, content, timestamp, tool_call_id, tool_name, tool_calls,
                reasoning, finish_reason, display_kind, display_metadata
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                message.session_id.as_str(),
                message.role,
                message.content,
                message.timestamp,
                message.tool_call_id,
                message.tool_name,
                message.tool_calls,
                message.reasoning,
                message.finish_reason,
                message.display_kind,
                message.display_metadata,
            ],
        )
        .context("写入消息失败")?;
    Ok(MessageId::new(transaction.last_insert_rowid()))
}

impl Store {
    /// 新建一个空会话。
    pub fn create_session(&mut self, session: &NewSession) -> Result<()> {
        self.ensure_writable()?;
        self.connection
            .execute(
                "INSERT INTO sessions (
                    id, source, model, title, started_at, last_activity_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?5)",
                params![
                    session.id.as_str(),
                    session.source,
                    session.model,
                    session.title,
                    session.started_at,
                ],
            )
            .context("创建会话失败")?;
        Ok(())
    }

    /// 原子地追加消息、递增计数并更新会话活跃时间。
    pub fn append_message(&mut self, message: &NewMessage) -> Result<MessageId> {
        self.ensure_writable()?;
        let transaction = self
            .connection
            .transaction()
            .context("开始消息追加事务失败")?;
        let id = insert_message(&transaction, message)?;
        let changed = transaction
            .execute(
                "UPDATE sessions
                 SET message_count = message_count + 1,
                     last_activity_at = ?1,
                     updated_at = ?1
                 WHERE id = ?2",
                params![message.timestamp, message.session_id.as_str()],
            )
            .context("更新会话消息计数失败")?;
        if changed != 1 {
            anyhow::bail!("消息所属会话不存在：{}", message.session_id.as_str());
        }
        transaction.commit().context("提交消息追加事务失败")?;
        Ok(id)
    }

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

    /// 更新会话的最近活动与更新时间；不存在时返回 false。
    pub fn update_session_activity(
        &mut self,
        session_id: &SessionId,
        timestamp: &str,
    ) -> Result<bool> {
        self.ensure_writable()?;
        let changed = self
            .connection
            .execute(
                "UPDATE sessions
                 SET last_activity_at = ?1, updated_at = ?1
                 WHERE id = ?2",
                params![timestamp, session_id.as_str()],
            )
            .context("更新会话活跃时间失败")?;
        Ok(changed == 1)
    }

    /// 修改会话标题；传入 None 会清除用户设置的标题。
    ///
    /// 不存在的会话返回 false，方便 TUI 处理其他窗口已删除会话的竞态。
    pub fn update_session_title(
        &mut self,
        session_id: &SessionId,
        title: Option<&str>,
        updated_at: &str,
    ) -> Result<bool> {
        self.ensure_writable()?;
        let changed = self
            .connection
            .execute(
                "UPDATE sessions
                 SET title = ?1, updated_at = ?2
                 WHERE id = ?3",
                params![title, updated_at, session_id.as_str()],
            )
            .context("更新会话标题失败")?;
        Ok(changed == 1)
    }

    /// 标记会话为结束状态，并持久化结束原因与结束时间。
    ///
    /// 空结束原因没有可解释的业务含义，因此在执行 SQL 前拒绝。
    pub fn finish_session(
        &mut self,
        session_id: &SessionId,
        end_reason: &str,
        ended_at: &str,
    ) -> Result<bool> {
        self.ensure_writable()?;
        if end_reason.trim().is_empty() {
            anyhow::bail!("会话结束原因不能为空");
        }
        let changed = self
            .connection
            .execute(
                "UPDATE sessions
                 SET ended_at = ?1, end_reason = ?2, updated_at = ?1
                 WHERE id = ?3",
                params![ended_at, end_reason, session_id.as_str()],
            )
            .context("结束会话失败")?;
        Ok(changed == 1)
    }

    /// 设置会话是否归档。归档会话仍可被 get_session 精确读取，但不会出现在列表中。
    pub fn set_session_archived(
        &mut self,
        session_id: &SessionId,
        archived: bool,
        updated_at: &str,
    ) -> Result<bool> {
        self.set_session_visibility(session_id, "archived", archived, updated_at)
    }

    /// 设置会话是否在普通会话列表中隐藏。
    pub fn set_session_hidden(
        &mut self,
        session_id: &SessionId,
        hidden: bool,
        updated_at: &str,
    ) -> Result<bool> {
        self.set_session_visibility(session_id, "hidden", hidden, updated_at)
    }

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

    /// 归档与隐藏只有列名不同；列名由本模块的固定常量给出，绝不接收外部输入。
    fn set_session_visibility(
        &mut self,
        session_id: &SessionId,
        column: &str,
        value: bool,
        updated_at: &str,
    ) -> Result<bool> {
        self.ensure_writable()?;
        debug_assert!(matches!(column, "archived" | "hidden"));
        let sql = format!("UPDATE sessions SET {column} = ?1, updated_at = ?2 WHERE id = ?3");
        let changed = self
            .connection
            .execute(
                &sql,
                params![i64::from(value), updated_at, session_id.as_str()],
            )
            .context("更新会话可见性失败")?;
        Ok(changed == 1)
    }
}
