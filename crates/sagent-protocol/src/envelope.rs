//! JSON-RPC 2.0 的通用信封类型。

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

/// JSON-RPC 请求标识。
///
/// 标识仅用于把响应关联到请求；通知省略 `id`。该类型保持 JSON-RPC 允许的字符串、
/// 数字和 `null` 三种线上表示，不把它转换为业务主键。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// 数字形式的客户端请求标识。
    Number(Number),
    /// 字符串形式的客户端请求标识。
    String(String),
    /// 客户端显式传入的空标识。
    Null,
}

/// 尚未分派到具体方法的 JSON-RPC 请求。
///
/// `params` 保留原始 JSON，仅在分派时反序列化为具体方法的强类型参数；它不应流入
/// 业务服务层。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// 协议版本，服务端只接受 `"2.0"`。
    pub jsonrpc: String,
    /// 缺失时表示通知，服务端不得为通知写出响应。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RequestId>,
    /// 要调用的方法名，例如 `gateway.ping`。
    pub method: String,
    /// 方法参数对象；省略时由方法自身决定是否允许。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 错误对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// 稳定的 JSON-RPC 或 Sagent 领域错误码。
    pub code: i32,
    /// 面向客户端的稳定英文短消息。
    pub message: String,
    /// 可选的安全上下文；不得写入密钥、数据库内容或回溯信息。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC 成功或失败响应。
///
/// 构造函数确保响应恰好包含 `result` 或 `error` 之一。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse<T> {
    /// 固定为 JSON-RPC 2.0。
    pub jsonrpc: String,
    /// 与请求对应的标识。
    pub id: RequestId,
    /// 成功结果。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    /// 失败详情。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl<T> JsonRpcResponse<T> {
    /// 创建成功响应。
    pub fn success(id: RequestId, result: T) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// 创建错误响应。
    pub fn failure(id: RequestId, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// 通知事件的参数信封。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventParams<T> {
    /// 事件类型，例如 `gateway.ready`。
    #[serde(rename = "type")]
    pub event_type: String,
    /// 事件所携带的结构化数据。
    pub payload: T,
}

/// 服务端主动发送的 JSON-RPC 事件。
///
/// 所有事件统一使用 `method: "event"`，具体事件名位于 `params.type`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcEvent<T> {
    /// 固定为 JSON-RPC 2.0。
    pub jsonrpc: String,
    /// 固定为 `event`。
    pub method: String,
    /// 事件类型与载荷。
    pub params: EventParams<T>,
}

impl<T> JsonRpcEvent<T> {
    /// 创建具有统一信封的新事件。
    pub fn new(event_type: impl Into<String>, payload: T) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            method: "event".to_owned(),
            params: EventParams {
                event_type: event_type.into(),
                payload,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{JsonRpcError, JsonRpcEvent, JsonRpcRequest, JsonRpcResponse, RequestId};

    #[test]
    fn request_preserves_a_notification_without_id() {
        let request: JsonRpcRequest = serde_json::from_value(json!({
            "jsonrpc": "2.0", "method": "gateway.ping", "params": {}
        }))
        .expect("通知请求应能反序列化");

        assert_eq!(request.id, None);
        assert_eq!(request.method, "gateway.ping");
    }

    #[test]
    fn success_and_error_responses_use_exclusive_fields() {
        let success = JsonRpcResponse::success(RequestId::Number(1.into()), json!({"ok": true}));
        assert_eq!(
            serde_json::to_value(success).expect("应能序列化"),
            json!({
                "jsonrpc": "2.0", "id": 1, "result": {"ok": true}
            })
        );

        let failure: JsonRpcResponse<Value> = JsonRpcResponse::failure(
            RequestId::String("request-2".to_owned()),
            JsonRpcError {
                code: -32601,
                message: "method not found".to_owned(),
                data: None,
            },
        );
        assert_eq!(
            serde_json::to_value(failure).expect("应能序列化"),
            json!({
                "jsonrpc": "2.0", "id": "request-2",
                "error": {"code": -32601, "message": "method not found"}
            })
        );
    }

    #[test]
    fn event_uses_the_shared_event_method_and_type_field() {
        let event = JsonRpcEvent::new("gateway.ready", json!({"protocol_version": 1}));
        assert_eq!(
            serde_json::to_value(event).expect("应能序列化"),
            json!({
                "jsonrpc": "2.0", "method": "event",
                "params": {"type": "gateway.ready", "payload": {"protocol_version": 1}}
            })
        );
    }
}
