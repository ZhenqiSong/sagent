//! JSON-RPC 错误码和错误对象。
//!
//! 定义标准 JSON-RPC 错误码和 Sagent 扩展错误码。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 错误码定义

use serde::{Deserialize, Serialize};

/// JSON-RPC 标准错误码（-32768 到 -32000）和 Sagent 扩展错误码（-32001 到 -32099）。
pub mod codes {
    /// 输入不是合法 JSON
    pub const PARSE_ERROR: i32 = -32700;
    /// 顶层 JSON 不是合法 JSON-RPC request
    pub const INVALID_REQUEST: i32 = -32600;
    /// method 未注册
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// params 缺少、类型错误或不满足 schema
    pub const INVALID_PARAMS: i32 = -32602;
    /// 未分类的服务端错误
    pub const INTERNAL_ERROR: i32 = -32603;

    /// 客户端要求不支持的协议版本
    pub const PROTOCOL_VERSION_UNSUPPORTED: i32 = -32001;
    /// 请求依赖未声明的 capability
    pub const CAPABILITY_UNSUPPORTED: i32 = -32002;
    /// 单行或单个 payload 超过限制
    pub const PAYLOAD_TOO_LARGE: i32 = -32003;
    /// 事件或请求序列违反协议约束
    pub const SEQUENCE_VIOLATION: i32 = -32004;
    /// 服务正在有序退出
    pub const SHUTDOWN: i32 = -32005;
}

/// JSON-RPC 错误对象。
///
/// 包含稳定的错误码和消息，可选 `data` 只能放机器可解析的安全诊断信息。
/// 不得把 stack trace、API key、完整环境变量或本地绝对路径放入 response。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorObject {
    /// 错误码（整数）
    pub code: i32,
    /// 稳定的错误消息
    pub message: String,
    /// 可选的诊断数据（仅机器可解析的安全信息）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ErrorObject {
    /// 创建一个 parse error（非法 JSON）。
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self {
            code: codes::PARSE_ERROR,
            message: message.into(),
            data: None,
        }
    }

    /// 创建一个 invalid request 错误。
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: codes::INVALID_REQUEST,
            message: message.into(),
            data: None,
        }
    }

    /// 创建一个 method not found 错误。
    pub fn method_not_found(method: impl Into<String>) -> Self {
        Self {
            code: codes::METHOD_NOT_FOUND,
            message: format!("Method not found: {}", method.into()),
            data: None,
        }
    }

    /// 创建一个 invalid params 错误。
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: codes::INVALID_PARAMS,
            message: message.into(),
            data: None,
        }
    }

    /// 创建一个 internal error。
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self {
            code: codes::INTERNAL_ERROR,
            message: message.into(),
            data: None,
        }
    }

    /// 创建一个 payload too large 错误。
    pub fn payload_too_large(message: impl Into<String>) -> Self {
        Self {
            code: codes::PAYLOAD_TOO_LARGE,
            message: message.into(),
            data: None,
        }
    }

    /// 创建一个 protocol version unsupported 错误。
    pub fn protocol_version_unsupported(message: impl Into<String>) -> Self {
        Self {
            code: codes::PROTOCOL_VERSION_UNSUPPORTED,
            message: message.into(),
            data: None,
        }
    }

    /// 创建一个 capability unsupported 错误。
    pub fn capability_unsupported(message: impl Into<String>) -> Self {
        Self {
            code: codes::CAPABILITY_UNSUPPORTED,
            message: message.into(),
            data: None,
        }
    }

    /// 创建一个 sequence violation 错误。
    pub fn sequence_violation(message: impl Into<String>) -> Self {
        Self {
            code: codes::SEQUENCE_VIOLATION,
            message: message.into(),
            data: None,
        }
    }

    /// 创建一个 shutdown 错误（服务正在有序退出）。
    pub fn shutdown(message: impl Into<String>) -> Self {
        Self {
            code: codes::SHUTDOWN,
            message: message.into(),
            data: None,
        }
    }
}
