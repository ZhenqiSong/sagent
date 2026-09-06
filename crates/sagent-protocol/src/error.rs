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
/// Sagent 错误码：交互方法尚未完成 `client.hello` 握手。
pub const HANDSHAKE_REQUIRED: i32 = -32006;
/// Sagent 错误码：客户端协议版本不受当前服务端支持。
pub const UNSUPPORTED_PROTOCOL_VERSION: i32 = -32007;
/// Sagent 错误码：连接没有声明调用该方法所需的 capability。
pub const CAPABILITY_NOT_GRANTED: i32 = -32008;
/// Sagent 错误码：同一会话已有活跃 Turn。
pub const SESSION_BUSY: i32 = -32009;
/// Sagent 错误码：目标会话没有可中断或可审批的活跃 Turn。
pub const NO_ACTIVE_TURN: i32 = -32010;
/// Sagent 错误码：当前 Profile 无法构建运行时所需依赖。
pub const RUNTIME_UNAVAILABLE: i32 = -32011;

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
    /// 会话数据库无法读取；细节仅保留在 stderr，不能暴露给协议客户端。
    #[error("store unavailable: {0}")]
    StoreUnavailable(String),
    /// 连接尚未完成 `client.hello`，不能调用交互方法。
    #[error("handshake required for method: {method}")]
    HandshakeRequired {
        /// 被拒绝的方法名。
        method: String,
    },
    /// 客户端声明的协议版本与服务端不兼容。
    #[error("unsupported protocol version: requested {requested}, supported {supported}")]
    UnsupportedProtocolVersion {
        /// 客户端请求的版本。
        requested: u32,
        /// 服务端当前支持的版本。
        supported: u32,
    },
    /// 连接没有声明调用目标方法所需的 capability。
    #[error("capability not granted: {capability} for method {method}")]
    CapabilityNotGranted {
        /// 缺失的 capability 名称。
        capability: String,
        /// 被拒绝的方法名。
        method: String,
    },
    /// 当前 Session 有尚未结束的 Turn，第一版不对 prompt 隐式排队。
    #[error("session is busy: {session_id}")]
    SessionBusy {
        /// 忙碌会话的标识。
        session_id: String,
    },
    /// 请求的 session/turn 当前没有活动执行目标。
    #[error("no active turn for session: {session_id}")]
    NoActiveTurn {
        /// 目标会话。
        session_id: String,
        /// 有则用于更精确地说明目标，但不暴露内部状态。
        turn_id: Option<String>,
    },
    /// RPC 启动时无法安全构建 Runtime；原始错误只写 stderr。
    #[error("runtime unavailable: {0}")]
    RuntimeUnavailable(String),
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
            Self::StoreUnavailable(_) => (STORE_UNAVAILABLE, "store unavailable".to_owned()),
            Self::HandshakeRequired { .. } => (HANDSHAKE_REQUIRED, "handshake required".to_owned()),
            Self::UnsupportedProtocolVersion { .. } => (
                UNSUPPORTED_PROTOCOL_VERSION,
                "unsupported protocol version".to_owned(),
            ),
            Self::CapabilityNotGranted { .. } => {
                (CAPABILITY_NOT_GRANTED, "capability not granted".to_owned())
            }
            Self::SessionBusy { .. } => (SESSION_BUSY, "session busy".to_owned()),
            Self::NoActiveTurn { .. } => (NO_ACTIVE_TURN, "no active turn".to_owned()),
            Self::RuntimeUnavailable(_) => (RUNTIME_UNAVAILABLE, "runtime unavailable".to_owned()),
            Self::Internal(_) => (INTERNAL_ERROR, "internal error".to_owned()),
        };

        let data = match self {
            Self::InvalidParams(detail) => Some(Value::String(detail.clone())),
            Self::MethodNotFound(method) => Some(serde_json::json!({ "method": method })),
            Self::SessionNotFound(session_id) => {
                Some(serde_json::json!({ "session_id": session_id }))
            }
            Self::HandshakeRequired { method } => Some(serde_json::json!({ "method": method })),
            Self::UnsupportedProtocolVersion {
                requested,
                supported,
            } => Some(serde_json::json!({
                "requested": requested,
                "supported": supported,
            })),
            Self::CapabilityNotGranted { capability, method } => Some(serde_json::json!({
                "capability": capability,
                "method": method,
            })),
            Self::SessionBusy { session_id } => {
                Some(serde_json::json!({ "session_id": session_id }))
            }
            Self::NoActiveTurn {
                session_id,
                turn_id,
            } => Some(serde_json::json!({
                "session_id": session_id,
                "turn_id": turn_id,
            })),
            Self::InvalidRequest
            | Self::StoreUnavailable(_)
            | Self::RuntimeUnavailable(_)
            | Self::Internal(_) => None,
        };

        JsonRpcError {
            code,
            message,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HANDSHAKE_REQUIRED, ProtocolError, STORE_UNAVAILABLE, UNSUPPORTED_PROTOCOL_VERSION,
    };

    #[test]
    fn store_unavailable_uses_stable_code_without_internal_detail() {
        let error = ProtocolError::StoreUnavailable("database path must stay private".to_owned())
            .to_jsonrpc();

        assert_eq!(error.code, STORE_UNAVAILABLE);
        assert_eq!(error.message, "store unavailable");
        assert_eq!(error.data, None);
    }

    #[test]
    fn handshake_errors_keep_method_and_version_details_stable() {
        let handshake = ProtocolError::HandshakeRequired {
            method: "prompt.submit".to_owned(),
        }
        .to_jsonrpc();
        let version = ProtocolError::UnsupportedProtocolVersion {
            requested: 2,
            supported: 1,
        }
        .to_jsonrpc();

        assert_eq!(handshake.code, HANDSHAKE_REQUIRED);
        assert_eq!(
            handshake.data,
            Some(serde_json::json!({"method": "prompt.submit"}))
        );
        assert_eq!(version.code, UNSUPPORTED_PROTOCOL_VERSION);
        assert_eq!(
            version.data,
            Some(serde_json::json!({"requested": 2, "supported": 1}))
        );
    }
}
