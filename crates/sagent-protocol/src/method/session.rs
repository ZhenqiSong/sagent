//! `session.*` 方法的协议类型。

use serde::{Deserialize, Serialize};

/// `session.list` 的分页和归档过滤参数。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionListParams {
    /// 是否包含已归档会话；默认不包含。
    #[serde(default)]
    pub include_archived: bool,
    /// 返回条数；服务层将在分派时应用默认值和最大值。
    #[serde(default)]
    pub limit: Option<u32>,
    /// 要跳过的会话数，默认从零开始。
    #[serde(default)]
    pub offset: u32,
}

/// 面向协议稳定性的会话摘要。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummaryDto {
    /// 会话的字符串标识。
    pub id: String,
    /// 会话创建来源，例如 `cli`。
    pub source: Option<String>,
    /// 会话绑定的模型标识。
    pub model: Option<String>,
    /// 面向用户的会话标题。
    pub title: Option<String>,
    /// 会话开始时间的 ISO-8601 字符串。
    pub started_at: Option<String>,
    /// 结束时间；未结束时为 `null`。
    pub ended_at: Option<String>,
    /// 结束原因；未结束时为 `null`。
    pub end_reason: Option<String>,
    /// 最近活动时间的 ISO-8601 字符串。
    pub last_active: Option<String>,
    /// 可用于会话列表的安全预览文本。
    pub preview: Option<String>,
    /// 当前可见消息数，而不是物理数据库总行数。
    pub message_count: i64,
}

/// `session.list` 的结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionListResult {
    /// 当前页的会话摘要。
    pub sessions: Vec<SessionSummaryDto>,
    /// 本次实际采用的分页大小。
    pub limit: u32,
    /// 本次查询跳过的条数。
    pub offset: u32,
}

/// `session.resume` 的参数。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResumeParams {
    /// 当前 Profile 中要读取的会话标识。
    pub session_id: String,
    /// 消息页大小；服务层将在分派时应用默认值和最大值。
    #[serde(default)]
    pub message_limit: Option<u32>,
    /// 要跳过的可见消息数。
    #[serde(default)]
    pub message_offset: u32,
}

/// 面向 UI 的可见消息 DTO。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMessageDto {
    /// SQLite 消息主键的线上数值表示。
    pub id: i64,
    /// 消息所属会话的字符串标识。
    pub session_id: String,
    /// OpenAI 风格消息角色，例如 `user` 或 `assistant`。
    pub role: String,
    /// 消息正文。
    pub content: String,
    /// 消息时间；旧数据缺失时为 `null`。
    pub timestamp: Option<String>,
    /// 关联工具调用标识；普通消息为 `null`。
    pub tool_call_id: Option<String>,
    /// 工具名称；普通消息为 `null`。
    pub tool_name: Option<String>,
    /// 序列化的工具调用载荷；没有工具调用时为 `null`。
    pub tool_calls: Option<String>,
    /// 模型推理文本；未持久化时为 `null`。
    pub reasoning: Option<String>,
    /// 模型结束原因；未知时为 `null`。
    pub finish_reason: Option<String>,
    /// UI 展示分类；默认消息为 `null`。
    pub display_kind: Option<String>,
    /// 展示分类的 JSON 元数据；不存在时为 `null`。
    pub display_metadata: Option<String>,
}

/// `session.resume` 返回的只读快照。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDetailDto {
    /// 已找到的会话摘要。
    pub session: SessionSummaryDto,
    /// 按时间正序排列、且符合可见性规则的消息。
    pub messages: Vec<SessionMessageDto>,
}

/// `session.resume` 的分页结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResumeResult {
    /// 会话与当前消息页的快照。
    #[serde(flatten)]
    pub detail: SessionDetailDto,
    /// 本次实际采用的消息页大小。
    pub message_limit: u32,
    /// 本次查询跳过的可见消息数。
    pub message_offset: u32,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{SessionListParams, SessionResumeParams};

    #[test]
    fn optional_pagination_parameters_default_without_losing_archived_filter() {
        let params: SessionListParams =
            serde_json::from_value(json!({})).expect("省略分页参数时应能反序列化");

        assert_eq!(params, SessionListParams::default());
    }

    #[test]
    fn resume_parameters_keep_the_required_session_id() {
        let params: SessionResumeParams =
            serde_json::from_value(json!({"session_id": "session-1"}))
                .expect("带会话标识的恢复参数应能反序列化");

        assert_eq!(params.session_id, "session-1");
        assert_eq!(params.message_limit, None);
        assert_eq!(params.message_offset, 0);
    }
}
