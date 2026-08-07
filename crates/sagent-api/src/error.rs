//! JSON-RPC 错误码和错误对象。
//!
//! 定义标准 JSON-RPC 错误码和 Sagent 扩展错误码。
//! 每个错误码只有一个定义来源：`ErrorCode` enum。
//! 相同输入错误在不同入口返回相同 code，不依赖错误字符串匹配。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 错误码定义
//! @change   2025-08-07 增强：添加 ErrorCode enum 提供类型安全映射

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

/// 错误码枚举。
///
/// 提供类型安全的错误码表示，支持与 i32 的双向转换。
/// 每个错误码只有一个定义来源。
///
/// # 示例
///
/// ```rust
/// use sagent_api::error::ErrorCode;
///
/// let code = ErrorCode::MethodNotFound;
/// assert_eq!(code.to_i32(), -32601);
/// assert_eq!(code.default_message(), "Method not found");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// 输入不是合法 JSON（-32700）
    ParseError,
    /// 顶层 JSON 不是合法 JSON-RPC request（-32600）
    InvalidRequest,
    /// method 未注册（-32601）
    MethodNotFound,
    /// params 缺少、类型错误或不满足 schema（-32602）
    InvalidParams,
    /// 未分类的服务端错误（-32603）
    InternalError,
    /// 客户端要求不支持的协议版本（-32001）
    ProtocolVersionUnsupported,
    /// 请求依赖未声明的 capability（-32002）
    CapabilityUnsupported,
    /// 单行或单个 payload 超过限制（-32003）
    PayloadTooLarge,
    /// 事件或请求序列违反协议约束（-32004）
    SequenceViolation,
    /// 服务正在有序退出（-32005）
    Shutdown,
}

impl ErrorCode {
    /// 返回错误码对应的整数值。
    pub fn to_i32(self) -> i32 {
        match self {
            Self::ParseError => codes::PARSE_ERROR,
            Self::InvalidRequest => codes::INVALID_REQUEST,
            Self::MethodNotFound => codes::METHOD_NOT_FOUND,
            Self::InvalidParams => codes::INVALID_PARAMS,
            Self::InternalError => codes::INTERNAL_ERROR,
            Self::ProtocolVersionUnsupported => codes::PROTOCOL_VERSION_UNSUPPORTED,
            Self::CapabilityUnsupported => codes::CAPABILITY_UNSUPPORTED,
            Self::PayloadTooLarge => codes::PAYLOAD_TOO_LARGE,
            Self::SequenceViolation => codes::SEQUENCE_VIOLATION,
            Self::Shutdown => codes::SHUTDOWN,
        }
    }

    /// 返回错误码对应的默认人类可读消息。
    pub fn default_message(self) -> &'static str {
        match self {
            Self::ParseError => "Parse error",
            Self::InvalidRequest => "Invalid Request",
            Self::MethodNotFound => "Method not found",
            Self::InvalidParams => "Invalid params",
            Self::InternalError => "Internal error",
            Self::ProtocolVersionUnsupported => "Protocol version unsupported",
            Self::CapabilityUnsupported => "Capability unsupported",
            Self::PayloadTooLarge => "Payload too large",
            Self::SequenceViolation => "Sequence violation",
            Self::Shutdown => "Server is shutting down",
        }
    }

    /// 从 i32 整数解析为 ErrorCode。
    ///
    /// 返回 `None` 表示未知的错误码。
    pub fn from_i32(code: i32) -> Option<Self> {
        match code {
            codes::PARSE_ERROR => Some(Self::ParseError),
            codes::INVALID_REQUEST => Some(Self::InvalidRequest),
            codes::METHOD_NOT_FOUND => Some(Self::MethodNotFound),
            codes::INVALID_PARAMS => Some(Self::InvalidParams),
            codes::INTERNAL_ERROR => Some(Self::InternalError),
            codes::PROTOCOL_VERSION_UNSUPPORTED => Some(Self::ProtocolVersionUnsupported),
            codes::CAPABILITY_UNSUPPORTED => Some(Self::CapabilityUnsupported),
            codes::PAYLOAD_TOO_LARGE => Some(Self::PayloadTooLarge),
            codes::SEQUENCE_VIOLATION => Some(Self::SequenceViolation),
            codes::SHUTDOWN => Some(Self::Shutdown),
            _ => None,
        }
    }

    /// 返回是否属于 JSON-RPC 标准错误码（-32768 到 -32000）。
    pub fn is_standard(self) -> bool {
        matches!(
            self,
            Self::ParseError
                | Self::InvalidRequest
                | Self::MethodNotFound
                | Self::InvalidParams
                | Self::InternalError
        )
    }

    /// 返回是否属于 Sagent 扩展错误码（-32001 到 -32099）。
    pub fn is_extension(self) -> bool {
        !self.is_standard()
    }
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
    /// 使用 ErrorCode 和默认消息创建错误对象。
    ///
    /// # 示例
    ///
    /// ```rust
    /// use sagent_api::error::{ErrorCode, ErrorObject};
    /// let err = ErrorObject::from_code(ErrorCode::ParseError);
    /// assert_eq!(err.code, -32700);
    /// ```
    pub fn from_code(code: ErrorCode) -> Self {
        Self {
            code: code.to_i32(),
            message: code.default_message().to_string(),
            data: None,
        }
    }

    /// 使用 ErrorCode 和自定义消息创建错误对象。
    pub fn from_code_with_message(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.to_i32(),
            message: message.into(),
            data: None,
        }
    }

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

    /// 为错误对象附加诊断数据。
    ///
    /// data 只能放机器可解析的安全诊断信息。
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}
