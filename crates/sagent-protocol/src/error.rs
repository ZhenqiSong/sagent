//! JSON-RPC 和 Sagent 协议层的统一错误定义。

use serde_json::Value;
use thiserror::Error;

use crate::envelope::JsonRpcError;

/// JSON-RPC 标准错误码：请求正文不是合法 JSON。
pub const PARSE_ERROR: i32 = -32700;
/// JSON-RPC 标准错误码：请求对象结构不合法。
pub const INVALID_REQUEST: i32 = -32600;
/// JSON-RPC 标准错误码：方法不存在。
pub const METHOD_NOT_FOUND: i32 = -32601;
/// JSON-RPC 标准错误码：方法参数不合法。
pub const INVALID_PARAMS: i32 = -32602;
/// JSON-RPC 标准错误码：未预期的内部错误。
pub const INTERNAL_ERROR: i32 = -32603;
/// Sagent 错误码：当前 Profile 中找不到会话。
pub const SESSION_NOT_FOUND: i32 = -32004;
/// Sagent 错误码：会话数据库不可读。
pub const STORE_UNAVAILABLE: i32 = -32005;

/// 协议校验和分派阶段的领域错误。
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// 请求的 `jsonrpc` 字段不是支持的版本。
    #[error("invalid JSON-RPC request: jsonrpc must be 2.0")]
    InvalidRequest,
    /// 方法参数无法反序列化或不满足方法约束。
    #[error("invalid params: {0}")]
    InvalidParams(String),
    /// 请求的方法名未注册。
    #[error("method not found: {0}")]
    MethodNotFound(String),
    /// 当前 Profile 中找不到请求的会话。
    #[error("session not found: {0}")]
    SessionNotFound(String),
    /// 服务层返回未预期的内部错误。
    #[error("internal error: {0}")]
    Internal(String),
}

impl ProtocolError {
    /// 转换为线上 JSON-RPC 错误对象。
    pub fn to_jsonrpc(&self) -> JsonRpcError {
        let (code, message) = match self {
            Self::InvalidRequest => (INVALID_REQUEST, "invalid request".to_owned()),
            Self::InvalidParams(_) => (INVALID_PARAMS, "invalid params".to_owned()),
            Self::MethodNotFound(_) => (METHOD_NOT_FOUND, "method not found".to_owned()),
            Self::SessionNotFound(_) => (SESSION_NOT_FOUND, "session not found".to_owned()),
            Self::Internal(_) => (INTERNAL_ERROR, "internal error".to_owned()),
        };

        let data = match self {
            Self::InvalidParams(detail) | Self::Internal(detail) => {
                Some(Value::String(detail.clone()))
            }
            Self::MethodNotFound(method) => Some(serde_json::json!({ "method": method })),
            Self::SessionNotFound(session_id) => {
                Some(serde_json::json!({ "session_id": session_id }))
            }
            Self::InvalidRequest => None,
        };

        JsonRpcError {
            code,
            message,
            data,
        }
    }
}
