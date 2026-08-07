//! JSON-RPC 方法分发器。
//!
//! 处理 Phase 0 的三类方法：rpc.echo、protocol.describe、health.get。
//! 对未知方法返回 MethodNotFound，对非法输入返回相应的标准 JSON-RPC 错误码。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 7 方法分发

use sagent_api::error::ErrorObject;
use sagent_types::ids::RequestId;
use sagent_types::version::{Capabilities, ProtocolVersion};
use tracing::{debug, warn};

/// 分发结果：成功返回 Some(JSON response)，通知返回 None。
///
/// 对于 notification（无 id 的请求），服务端不返回 response。
pub type DispatchResult = Result<Option<serde_json::Value>, (RequestId, ErrorObject)>;

/// 分发单个 JSON-RPC 请求。
///
/// 处理流程：
/// 1. 验证 JSON-RPC envelope（jsonrpc 版本、method 存在性）
/// 2. 验证 method 是否在 capability 列表中
/// 3. 分发到对应的 handler
///
/// 返回 `Ok(None)` 表示 notification（不返回 response）。
/// 返回 `Ok(Some(json))` 表示成功响应。
/// 返回 `Err((id, error))` 表示错误响应。
pub fn dispatch(line: &str, caps: &Capabilities) -> DispatchResult {
    // Step 1: 解析 JSON
    let value: serde_json::Value = serde_json::from_str(line).map_err(|e| {
        debug!("JSON 解析失败: {}", e);
        // Parse error 时无法获取 request id，使用 null
        (
            RequestId::String("null".to_string()),
            ErrorObject::parse_error(format!("Parse error: {}", e)),
        )
    })?;

    // Step 2: 验证 jsonrpc 版本
    let jsonrpc = value.get("jsonrpc").and_then(|v| v.as_str()).unwrap_or("");
    if jsonrpc != "2.0" {
        let id = extract_id(&value);
        return Err((id, ErrorObject::invalid_request("jsonrpc must be \"2.0\"")));
    }

    // Step 3: 验证 method 存在
    let method = match value.get("method").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => {
            let id = extract_id(&value);
            return Err((id, ErrorObject::invalid_request("missing method")));
        },
    };

    // Step 4: 判断是 notification 还是 request
    let is_notification = !value.get("id").is_some_and(|v| !v.is_null());
    let request_id = extract_id(&value);

    // Step 5: 提取 params
    let params = value
        .get("params")
        .cloned()
        .unwrap_or(serde_json::Value::Object(Default::default()));

    // Step 6: 验证 params 是 object
    if !params.is_object() {
        return Err((
            request_id,
            ErrorObject::invalid_params("params must be a JSON object"),
        ));
    }

    // Step 7: 验证 method 是否在 capability 列表中
    if !caps.validate_method(&method) {
        warn!("未知方法: {}", method);
        return Err((request_id, ErrorObject::method_not_found(&method)));
    }

    // Step 8: 分发
    let result = match method.as_str() {
        "rpc.echo" => handle_echo(&params),
        "protocol.describe" => handle_protocol_describe(),
        "health.get" => handle_health(),
        _ => unreachable!("method 已在 capability 校验中被拦截"),
    };

    match result {
        Ok(result_value) => {
            if is_notification {
                debug!("notification 不返回 response: {}", method);
                Ok(None)
            } else {
                let response = build_success_response(request_id, result_value);
                Ok(Some(response))
            }
        },
        Err(err_obj) => Err((request_id, err_obj)),
    }
}

/// 从 JSON value 中提取 request id。
///
/// 返回 RequestId，无法提取时使用 null 字符串。
fn extract_id(value: &serde_json::Value) -> RequestId {
    match value.get("id") {
        Some(serde_json::Value::String(s)) => RequestId::String(s.clone()),
        Some(serde_json::Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                RequestId::Number(i)
            } else {
                RequestId::String("null".to_string())
            }
        },
        _ => RequestId::String("null".to_string()),
    }
}

/// 构建成功响应。
fn build_success_response(id: RequestId, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

/// 构建错误响应。
pub fn build_error_response(id: RequestId, error: &ErrorObject) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": error.code,
            "message": error.message,
            "data": error.data,
        }
    })
}

/// 处理 rpc.echo：原样返回 params。
fn handle_echo(params: &serde_json::Value) -> Result<serde_json::Value, ErrorObject> {
    Ok(params.clone())
}

/// 处理 protocol.describe：返回协议版本和 capabilities。
fn handle_protocol_describe() -> Result<serde_json::Value, ErrorObject> {
    let pv = ProtocolVersion::default();
    let caps = Capabilities::default();
    Ok(serde_json::json!({
        "protocol": pv.protocol,
        "version": pv.version,
        "runtime_version": pv.runtime_version,
        "features": caps.feature_names(),
    }))
}

/// 处理 health.get：返回健康状态。
fn handle_health() -> Result<serde_json::Value, ErrorObject> {
    let pv = ProtocolVersion::default();
    Ok(serde_json::json!({
        "status": "ok",
        "protocol": pv.protocol,
        "version": pv.version,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_caps() -> Capabilities {
        Capabilities::default()
    }

    #[test]
    fn test_rpc_echo_returns_params() {
        let caps = make_caps();
        let input = r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{"value":"hello"}}"#;
        let result = dispatch(input, &caps);
        assert!(result.is_ok());
        let response = result.unwrap().unwrap();
        assert_eq!(response["id"], "1");
        assert_eq!(response["result"]["value"], "hello");
    }

    #[test]
    fn test_rpc_echo_with_number_id() {
        let caps = make_caps();
        let input = r#"{"jsonrpc":"2.0","id":42,"method":"rpc.echo","params":{"x":1}}"#;
        let result = dispatch(input, &caps);
        assert!(result.is_ok());
        let response = result.unwrap().unwrap();
        assert_eq!(response["id"], 42);
        assert_eq!(response["result"]["x"], 1);
    }

    #[test]
    fn test_notification_returns_none() {
        let caps = make_caps();
        let input = r#"{"jsonrpc":"2.0","method":"rpc.echo","params":{"value":"hello"}}"#;
        let result = dispatch(input, &caps);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_protocol_describe() {
        let caps = make_caps();
        let input = r#"{"jsonrpc":"2.0","id":"1","method":"protocol.describe","params":{}}"#;
        let result = dispatch(input, &caps);
        assert!(result.is_ok());
        let response = result.unwrap().unwrap();
        assert_eq!(response["result"]["protocol"], "sagent.rpc");
        assert_eq!(response["result"]["version"], 1);
        assert!(response["result"]["features"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("rpc.echo")));
    }

    #[test]
    fn test_health_get() {
        let caps = make_caps();
        let input = r#"{"jsonrpc":"2.0","id":"1","method":"health.get","params":{}}"#;
        let result = dispatch(input, &caps);
        assert!(result.is_ok());
        let response = result.unwrap().unwrap();
        assert_eq!(response["result"]["status"], "ok");
    }

    #[test]
    fn test_invalid_json_returns_parse_error() {
        let caps = make_caps();
        let input = "not valid json";
        let result = dispatch(input, &caps);
        assert!(result.is_err());
        let (_, err) = result.unwrap_err();
        assert_eq!(err.code, -32700);
    }

    #[test]
    fn test_missing_jsonrpc_returns_invalid_request() {
        let caps = make_caps();
        let input = r#"{"id":"1","method":"rpc.echo","params":{}}"#;
        let result = dispatch(input, &caps);
        assert!(result.is_err());
        let (_, err) = result.unwrap_err();
        assert_eq!(err.code, -32600);
    }

    #[test]
    fn test_wrong_jsonrpc_version_returns_invalid_request() {
        let caps = make_caps();
        let input = r#"{"jsonrpc":"1.0","id":"1","method":"rpc.echo","params":{}}"#;
        let result = dispatch(input, &caps);
        assert!(result.is_err());
        let (_, err) = result.unwrap_err();
        assert_eq!(err.code, -32600);
    }

    #[test]
    fn test_missing_method_returns_invalid_request() {
        let caps = make_caps();
        let input = r#"{"jsonrpc":"2.0","id":"1","params":{}}"#;
        let result = dispatch(input, &caps);
        assert!(result.is_err());
        let (_, err) = result.unwrap_err();
        assert_eq!(err.code, -32600);
    }

    #[test]
    fn test_unknown_method_returns_method_not_found() {
        let caps = make_caps();
        let input = r#"{"jsonrpc":"2.0","id":"1","method":"session.create","params":{}}"#;
        let result = dispatch(input, &caps);
        assert!(result.is_err());
        let (_, err) = result.unwrap_err();
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn test_params_is_array_returns_invalid_params() {
        let caps = make_caps();
        let input = r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":["a","b"]}"#;
        let result = dispatch(input, &caps);
        assert!(result.is_err());
        let (_, err) = result.unwrap_err();
        assert_eq!(err.code, -32602);
    }

    #[test]
    fn test_params_is_string_returns_invalid_params() {
        let caps = make_caps();
        let input = r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":"not-object"}"#;
        let result = dispatch(input, &caps);
        assert!(result.is_err());
        let (_, err) = result.unwrap_err();
        assert_eq!(err.code, -32602);
    }

    #[test]
    fn test_error_response_contains_request_id() {
        let caps = make_caps();
        let input = r#"{"jsonrpc":"2.0","id":"my-id","method":"unknown.method","params":{}}"#;
        let result = dispatch(input, &caps);
        assert!(result.is_err());
        let (id, _) = result.unwrap_err();
        assert_eq!(id, RequestId::String("my-id".to_string()));
    }

    #[test]
    fn test_build_error_response_format() {
        let id = RequestId::String("req-1".to_string());
        let err = ErrorObject::method_not_found("test");
        let resp = build_error_response(id, &err);
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], "req-1");
        assert_eq!(resp["error"]["code"], -32601);
        assert!(resp["error"]["message"].as_str().unwrap().contains("test"));
    }

    #[test]
    fn test_two_consecutive_requests() {
        let caps = make_caps();
        let input1 = r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{"a":1}}"#;
        let input2 = r#"{"jsonrpc":"2.0","id":"2","method":"rpc.echo","params":{"b":2}}"#;

        let r1 = dispatch(input1, &caps).unwrap().unwrap();
        let r2 = dispatch(input2, &caps).unwrap().unwrap();

        assert_eq!(r1["id"], "1");
        assert_eq!(r1["result"]["a"], 1);
        assert_eq!(r2["id"], "2");
        assert_eq!(r2["result"]["b"], 2);
    }
}
