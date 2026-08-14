//! Phase 1 Session 数据模型。
//!
//! Session 只描述持久化身份和状态，不包含 Agent 执行状态、Provider 或工具运行时字段。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Session 数据模型

use serde::{Deserialize, Serialize};

use crate::ids::SessionId;

/// Session 的持久化生命周期状态。
///
/// 这些状态不是 Agent 执行状态；Phase 1 不定义 thinking、waiting_for_tool 或 compressing。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    /// Session 可以接受新的持久化操作。
    Active,
    /// Session 已正常关闭。
    Closed,
    /// Session 正在从数据库恢复。
    Recovering,
}

/// 可恢复的 Session 持久化模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// 服务端生成且不可变的 Session ID。
    pub id: SessionId,
    /// 创建来源，例如 `cli` 或 `stdio`。
    pub source: String,
    /// 用户可见标题。
    pub title: Option<String>,
    /// 创建时间（RFC 3339 UTC）。
    pub created_at: String,
    /// 最后一次成功持久化变更时间（RFC 3339 UTC）。
    pub updated_at: String,
    /// Session 生命周期状态。
    pub status: SessionStatus,
    /// 创建时的工作目录，仅保存，不自动验证。
    pub cwd: Option<String>,
    /// Session 扩展 metadata；不承载 secret。
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
    /// 已提交消息数量，要求与数据库中的 active message 数量一致。
    pub message_count: u64,
    /// 成功提交状态变更后的单调递增版本。
    pub revision: u64,
}

impl Session {
    /// 校验 Session 的基本持久化不变量。
    pub fn validate(&self) -> Result<(), SessionValidationError> {
        if self.id.0.is_empty() {
            return Err(SessionValidationError::EmptyId);
        }
        if self.source.is_empty() {
            return Err(SessionValidationError::EmptySource);
        }
        if self.created_at.is_empty() || self.updated_at.is_empty() {
            return Err(SessionValidationError::EmptyTimestamp);
        }
        Ok(())
    }

    /// 返回追加一条已提交消息后的投影状态。
    ///
    /// Repository 只有在 SQLite 事务成功后才应使用该方法更新内存快照。
    pub fn after_message_commit(&self, updated_at: String) -> Self {
        let mut next = self.clone();
        next.updated_at = updated_at;
        next.message_count = next.message_count.saturating_add(1);
        next.revision = next.revision.saturating_add(1);
        next
    }

    /// 返回成功关闭后的投影状态。
    pub fn after_close_commit(&self, updated_at: String) -> Self {
        let mut next = self.clone();
        next.updated_at = updated_at;
        next.status = SessionStatus::Closed;
        next.revision = next.revision.saturating_add(1);
        next
    }
}

/// Session 持久化字段校验错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionValidationError {
    /// ID 不能为空。
    EmptyId,
    /// source 不能为空。
    EmptySource,
    /// 时间戳不能为空。
    EmptyTimestamp,
}

impl std::fmt::Display for SessionValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::EmptyId => "id 不能为空",
            Self::EmptySource => "source 不能为空",
            Self::EmptyTimestamp => "created_at 和 updated_at 不能为空",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for SessionValidationError {}
