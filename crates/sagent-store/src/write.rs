//! 会话和消息的最小写入接口。
//!
//! 作者：SongZQ

use anyhow::{Context, Result};
use rusqlite::{Transaction, params};
use sagent_types::{MessageId, SessionId, StoredMessage};

use crate::Store;

// 这两个子模块仍属于 write：它们共享 Store 的事务与只读保护，只是分别承载“替换
// 活动上下文”和“回退旧分支”两种生命周期，避免常规追加接口演变成写入 god-file。
#[path = "write/branch.rs"]
mod branch;
#[path = "write/context.rs"]
mod context;

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
    /// Provider 工具调用 ID；仅 tool 消息使用。
    pub tool_call_id: Option<String>,
    /// 工具名称；普通消息为空。
    pub tool_name: Option<String>,
    /// assistant 发起的工具调用 JSON。
    pub tool_calls: Option<String>,
    /// Provider 推理文本。
    pub reasoning: Option<String>,
    /// Provider 的完成原因。
    pub finish_reason: Option<String>,
    /// 展示层消息分类。
    pub display_kind: Option<String>,
    /// 展示层附加 JSON 元数据。
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

/// 一次跨进程恢复操作的结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreResult {
    /// 从 inactive 重新变为 active 的消息数量。
    pub restored_count: u64,
    /// 恢复后最后一条活动消息；会话没有消息时为 None。
    pub new_head_id: Option<MessageId>,
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
pub(crate) fn insert_message(
    transaction: &Transaction<'_>,
    message: &NewMessage,
) -> Result<MessageId> {
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

    /// 在同一事务中按给定顺序追加同一会话的一批消息。
    ///
    /// 此入口用于导入和大规模 fixture：逐条调用 [`Self::append_message`] 会为每条消息
    /// 提交一次事务，既放大 SQLite fsync 开销，也会让中途失败留下部分可见数据。调用方
    /// 必须传入同一会话且按时间顺序排列的消息；空集合不触碰数据库。成功时计数和活跃
    /// 时间只更新一次，失败时消息与会话元数据一并回滚。
    pub fn append_messages(&mut self, messages: &[NewMessage]) -> Result<Vec<MessageId>> {
        self.ensure_writable()?;
        let Some(first) = messages.first() else {
            return Ok(Vec::new());
        };
        if messages
            .iter()
            .any(|message| message.session_id != first.session_id)
        {
            anyhow::bail!("批量追加的消息必须属于同一会话");
        }
        // 已确认 first 存在，因此 last 也必须存在；仍转换为 Result，避免存储层用 panic
        // 表达可恢复错误路径。
        let last_timestamp = messages
            .last()
            .map(|message| &message.timestamp)
            .context("非空批量消息缺少末尾时间戳")?;

        let transaction = self
            .connection
            .transaction()
            .context("开始批量消息追加事务失败")?;
        let mut ids = Vec::with_capacity(messages.len());
        for message in messages {
            ids.push(insert_message(&transaction, message)?);
        }
        let changed = transaction
            .execute(
                "UPDATE sessions
                 SET message_count = message_count + ?1,
                     last_activity_at = ?2,
                     updated_at = ?2
                 WHERE id = ?3",
                params![
                    i64::try_from(messages.len()).context("批量消息数量超过 SQLite 整数范围")?,
                    last_timestamp,
                    first.session_id.as_str(),
                ],
            )
            .context("更新批量消息计数失败")?;
        if changed != 1 {
            anyhow::bail!("消息所属会话不存在：{}", first.session_id.as_str());
        }
        transaction.commit().context("提交批量消息追加事务失败")?;
        Ok(ids)
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
