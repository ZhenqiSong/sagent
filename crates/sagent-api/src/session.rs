//! Session JSON-RPC 参数类型。

use sagent_types::ids::SessionId;
use sagent_types::session::SessionStatus;
use serde::{Deserialize, Serialize};

/// 稳定的 Session 列表游标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// 游标记录的更新时间。
    pub updated_at: String,
    /// 游标记录的 ID。
    pub id: SessionId,
}

/// `session.create` 参数。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateParams {
    /// 创建来源；缺省时使用 `stdio`。
    pub source: Option<String>,
    /// 用户可见标题。
    pub title: Option<String>,
    /// 保存到 Session 的工作目录。
    pub cwd: Option<String>,
    /// 扩展 metadata。
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

/// `session.list` 参数。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    /// 最大返回数量。
    pub limit: Option<u32>,
    /// 稳定分页游标。
    pub before: Option<Cursor>,
    /// 来源过滤。
    pub source: Option<String>,
    /// 状态过滤。
    pub status: Option<SessionStatus>,
}

/// `session.get` 参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetParams {
    /// Session ID。
    pub session_id: SessionId,
    /// 只返回此 sequence 之后的消息。
    pub after_sequence: Option<u64>,
    /// 消息窗口大小。
    pub limit: Option<u32>,
}

/// `session.resume` 参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeParams {
    /// Session ID。
    pub session_id: SessionId,
}

/// `session.subscribe` 参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscribeParams {
    /// Session ID。
    pub session_id: SessionId,
    /// 客户端已知的事件序号；Phase 1 不提供 durable event replay。
    #[serde(default)]
    pub after_seq: u64,
}
