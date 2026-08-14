//! Repository 输入、查询和恢复模型。
//!
//! 这些类型隔离 SQL 行结构和上层 Session/Message 数据模型。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 4 Repository models

use sagent_types::ids::{SessionId, ToolCallId};
use sagent_types::message::{ContentPart, Role};
use sagent_types::session::{Session, SessionStatus};
use sagent_types::tool::ToolCall;

/// 创建 Session 的输入。
#[derive(Debug, Clone)]
pub struct CreateSession {
    /// 创建来源。
    pub source: String,
    /// 可选标题。
    pub title: Option<String>,
    /// 可选工作目录，仅保存不验证。
    pub cwd: Option<String>,
    /// 扩展 metadata。
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl CreateSession {
    /// 使用来源创建空 metadata 的输入。
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            title: None,
            cwd: None,
            metadata: Default::default(),
        }
    }
}

/// 追加 Message 的输入。
#[derive(Debug, Clone)]
pub struct AppendMessage {
    /// 消息角色。
    pub role: Role,
    /// 消息内容。
    pub content: Vec<ContentPart>,
    /// 工具调用列表。
    pub tool_calls: Vec<ToolCall>,
    /// 工具结果关联的 tool call ID。
    pub tool_call_id: Option<ToolCallId>,
    /// 消展扩展 metadata。
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl AppendMessage {
    /// 创建文本消息输入。
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![ContentPart::Text { text: text.into() }],
            tool_calls: vec![],
            tool_call_id: None,
            metadata: Default::default(),
        }
    }
}

/// Session 列表查询。
#[derive(Debug, Clone, Default)]
pub struct ListSessions {
    /// 最大返回数量。
    pub limit: Option<u32>,
    /// 稳定分页游标；游标之前的记录不会返回。
    pub before: Option<SessionCursor>,
    /// 来源过滤。
    pub source: Option<String>,
    /// 状态过滤。
    pub status: Option<SessionStatus>,
}

/// Session 列表稳定游标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCursor {
    /// 游标记录的 updated_at。
    pub updated_at: String,
    /// 游标记录的 ID。
    pub id: SessionId,
}

/// Message 分页范围。
#[derive(Debug, Clone, Default)]
pub struct MessageRange {
    /// 最大返回数量。
    pub limit: Option<u32>,
    /// 只返回 sequence 大于此值的消息。
    pub after_sequence: Option<u64>,
}

/// Session 列表的轻量投影。
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// Session ID。
    pub id: SessionId,
    /// 创建来源。
    pub source: String,
    /// 标题。
    pub title: Option<String>,
    /// 最后更新时间。
    pub updated_at: String,
    /// 生命周期状态。
    pub status: SessionStatus,
    /// 已提交消息数。
    pub message_count: u64,
    /// 当前 revision。
    pub revision: u64,
}

impl From<Session> for SessionSummary {
    fn from(session: Session) -> Self {
        Self {
            id: session.id,
            source: session.source,
            title: session.title,
            updated_at: session.updated_at,
            status: session.status,
            message_count: session.message_count,
            revision: session.revision,
        }
    }
}

/// 恢复时返回的 Session 和受限消息窗口。
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    /// Session 元数据。
    pub session: Session,
    /// 按 sequence 升序排列的消息。
    pub messages: Vec<sagent_types::message::Message>,
}

/// Repository 支持的最大列表数量。
pub const MAX_LIST_LIMIT: u32 = 200;
/// Repository 支持的最大消息窗口数量。
pub const MAX_MESSAGE_LIMIT: u32 = 10_000;
