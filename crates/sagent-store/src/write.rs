//! 会话和消息的最小写入接口。
//!
//! 作者：SongZQ

use anyhow::{Context, Result};
use rusqlite::params;
use sagent_types::{MessageId, SessionId};

use crate::Store;

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
        let id = MessageId::new(transaction.last_insert_rowid());
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
