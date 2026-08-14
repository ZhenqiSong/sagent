//! JSON-RPC request 类型。
//!
//! 定义 JSON-RPC 2.0 的 request 结构和 RequestId。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 request/notification 类型
//! @change   2026-08-14 增强：缺省 params 统一反序列化为空对象

use sagent_types::ids::RequestId;
use serde::{Deserialize, Serialize};

/// 返回协议约定的空 params 对象。
fn default_params() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

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
    #[serde(default = "default_params")]
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
    #[serde(default = "default_params")]
    pub params: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::{Notification, Request};

    #[test]
    fn missing_params_defaults_to_empty_object() {
        let request: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo"}"#)
                .expect("request 反序列化失败");
        assert_eq!(request.params, serde_json::json!({}));

        let notification: Notification =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"rpc.echo"}"#)
                .expect("notification 反序列化失败");
        assert_eq!(notification.params, serde_json::json!({}));
    }
}
