//! JSON-RPC request 类型。
//!
//! 定义 JSON-RPC 2.0 的 request 结构和 RequestId。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 request/notification 类型

use sagent_types::ids::RequestId;
use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 请求。
///
/// 请求必须包含 `jsonrpc: "2.0"`、`id`、`method`。
/// `params` 必须是 JSON object，没有参数时使用 `{}`。
/// 协议 envelope 级别拒绝未知字段，防止协议不兼容。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// JSON-RPC 版本，固定为 "2.0"
    pub jsonrpc: String,
    /// 请求 ID（string 或 number）
    pub id: RequestId,
    /// 方法名
    pub method: String,
    /// 方法参数（必须是 object）
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 2.0 通知（无 id 字段的请求）。
///
/// 通知不期望 response，服务端不返回任何内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notification {
    /// JSON-RPC 版本，固定为 "2.0"
    pub jsonrpc: String,
    /// 方法名
    pub method: String,
    /// 方法参数（必须是 object）
    #[serde(default)]
    pub params: serde_json::Value,
}
