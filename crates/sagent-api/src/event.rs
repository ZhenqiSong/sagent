//! Event envelope 和事件类型。
//!
//! 定义跨进程事件通知的标准格式。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 事件 envelope 定义

use sagent_types::ids::{EventId, SessionId, TurnId};
use serde::{Deserialize, Serialize};

/// 事件 envelope。
///
/// 所有通过 stdio/WebSocket 发送的事件通知都使用此格式。
/// 事件是 JSON-RPC notification（不带 `id` 字段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventEnvelope {
    /// JSON-RPC 版本，固定为 "2.0"
    pub jsonrpc: String,
    /// 事件方法名（如 "message.delta"）
    pub method: String,
    /// 事件参数
    pub params: EventParams,
}

/// 事件参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventParams {
    /// 事件唯一标识（当前 event stream 内唯一）
    pub event_id: EventId,
    /// Session ID（session 事件必须有；全局事件可省略）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Turn ID（turn 相关事件必须有）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// 事件序号（从 1 开始，按 stream 严格递增）
    pub seq: u64,
    /// 事件时间戳（RFC 3339 UTC）
    pub timestamp: String,
    /// 事件 payload（事件类型对应数据）
    pub data: serde_json::Value,
}
