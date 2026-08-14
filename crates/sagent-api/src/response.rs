//! JSON-RPC response 类型。
//!
//! 定义 JSON-RPC 2.0 的 response 结构。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 response 类型

use sagent_types::ids::RequestId;
use serde::{Deserialize, Serialize};

use crate::error::ErrorObject;

/// JSON-RPC 2.0 成功响应。
///
/// response 必须包含 result 或 error 二者之一，不可同时存在或同时缺失。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuccessResponse {
    /// JSON-RPC 版本，固定为 "2.0"
    pub jsonrpc: String,
    /// 对应的请求 ID
    pub id: RequestId,
    /// 成功结果
    pub result: serde_json::Value,
}

/// JSON-RPC 2.0 错误响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    /// JSON-RPC 版本，固定为 "2.0"
    pub jsonrpc: String,
    /// 对应的请求 ID
    pub id: Option<RequestId>,
    /// 错误对象
    pub error: ErrorObject,
}
