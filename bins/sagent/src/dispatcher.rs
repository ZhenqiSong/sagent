//! JSON-RPC 方法分发器。
//!
//! 处理 Phase 0 的三类方法：rpc.echo、protocol.describe、health.get。
//! 对未知方法返回 MethodNotFound，对非法输入返回相应的标准 JSON-RPC 错误码。
//!
//! 每个 RPC request 都携带 `request_id` tracing span，确保日志可关联。
//! 日志中不打印完整 request params，仅输出脱敏后的摘要。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 7 方法分发
//! @change   2025-08-12 增强：Phase 0 Step 9 request_id span、结构化日志、敏感数据过滤
//! @change   2026-08-14 增强：严格校验 envelope、method 和 request id 边界

use sagent_api::error::ErrorObject;
use sagent_api::logging;
use sagent_types::ids::RequestId;
use sagent_types::version::{Capabilities, ProtocolVersion};
use tracing::{debug, error, info, warn};

use crate::stdio::{MAX_ID_BYTES, MAX_METHOD_BYTES};

/// 分发结果：成功返回 Some(JSON response)，通知返回 None。
///
/// 对于 notification（无 id 的请求），服务端不返回 response。
pub type DispatchResult = Result<Option<serde_json::Value>, (Option<RequestId>, ErrorObject)>;

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
        error!(error = %e, "JSON 解析失败");
        (
            None,
            ErrorObject::parse_error(format!("Parse error: {}", e)),
        )
    })?;

    // Step 2: 验证顶层 envelope
    let object = match value.as_object() {
        Some(object) => object,
        None => {
            warn!("JSON-RPC 顶层值不是 object");
            return Err((
                None,
                ErrorObject::invalid_request("request must be an object"),
            ));
        },
    };
    const ALLOWED_FIELDS: &[&str] = &["jsonrpc", "id", "method", "params"];
    if let Some(field) = object.keys().find(|field| !ALLOWED_FIELDS.contains(&field.as_str())) {
        let id = extract_id(&value);
        warn!(field = %field, request_id = %format_request_id(&id), "JSON-RPC envelope 包含未知字段");
        return Err((id, ErrorObject::invalid_request("unknown envelope field")));
    }

    // Step 3: 验证 jsonrpc 版本和 request id
    let jsonrpc = value.get("jsonrpc").and_then(|v| v.as_str()).unwrap_or("");
    if jsonrpc != "2.0" {
        let id = extract_id(&value);
        warn!(request_id = %format_request_id(&id), jsonrpc = %jsonrpc, "非法 jsonrpc 版本");
        return Err((id, ErrorObject::invalid_request("jsonrpc must be \"2.0\"")));
    }

    if let Some(id_value) = value.get("id") {
        if !is_valid_request_id(id_value) {
            let id = extract_id(&value);
            warn!(request_id = %format_request_id(&id), "request id 类型非法");
            return Err((
                None,
                ErrorObject::invalid_request("id must be a string or integer"),
            ));
        }
        if request_id_bytes(id_value) > MAX_ID_BYTES {
            let id = extract_id(&value);
            warn!(request_id = %format_request_id(&id), "request id 超过长度限制");
            return Err((
                id,
                ErrorObject::payload_too_large("request id exceeds 256 bytes"),
            ));
        }
    }

    // Step 3: 验证 method 存在
    let method = match value.get("method").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => {
            let id = extract_id(&value);
            warn!(request_id = %format_request_id(&id), "缺少 method 字段");
            return Err((id, ErrorObject::invalid_request("missing method")));
        },
    };

    if method.is_empty() {
        let id = extract_id(&value);
        warn!(request_id = %format_request_id(&id), "method 不能为空");
        return Err((id, ErrorObject::invalid_request("method must not be empty")));
    }
    if method.len() > MAX_METHOD_BYTES {
        let id = extract_id(&value);
        warn!(request_id = %format_request_id(&id), "method 超过长度限制");
        return Err((
            id,
            ErrorObject::payload_too_large("method exceeds 256 bytes"),
        ));
    }

    // Step 4: 判断是 notification 还是 request
    let is_notification = !object.contains_key("id");
    let request_id = extract_id(&value);

    // 创建 request span，整个处理过程关联到该 span
    let span = logging::request_span(&format_request_id(&request_id));
    let _guard = span.enter();

    debug!(
        request_id = %format_request_id(&request_id),
        method = %method,
        is_notification = is_notification,
        "收到 JSON-RPC 请求"
    );

    // Step 5: 提取 params（脱敏后用于日志）
    let params = value
        .get("params")
        .cloned()
        .unwrap_or(serde_json::Value::Object(Default::default()));

    // 在日志中使用脱敏后的 params，避免泄露敏感数据
    let safe_params = logging::redact_sensitive(&params);
    debug!(
        request_id = %format_request_id(&request_id),
        method = %method,
        params = %safe_params,
        "请求参数（已脱敏）"
    );

    // Step 6: 验证 params 是 object
    if !params.is_object() {
        warn!(
            request_id = %format_request_id(&request_id),
            method = %method,
            "params 不是 JSON object"
        );
        return Err((
            request_id.clone(),
            ErrorObject::invalid_params("params must be a JSON object"),
        ));
    }

    // Step 7: 验证 method 是否在 capability 列表中
    if !caps.validate_method(&method) {
        warn!(
            request_id = %format_request_id(&request_id),
            method = %method,
            "未知方法"
        );
        return Err((request_id.clone(), ErrorObject::method_not_found(&method)));
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
                info!(
                    request_id = %format_request_id(&request_id),
                    method = %method,
                    "notification 处理完成，不返回 response"
                );
                Ok(None)
            } else {
                info!(
                    request_id = %format_request_id(&request_id),
                    method = %method,
                    "请求处理成功"
                );
                let response = build_success_response(
                    request_id.expect("request 必须包含有效 id"),
                    result_value,
                );
                Ok(Some(response))
            }
        },
        Err(err_obj) => {
            error!(
                request_id = %format_request_id(&request_id),
                method = %method,
                error_code = err_obj.code,
                error_message = %err_obj.message,
                "请求处理失败"
            );
            Err((request_id, err_obj))
        },
    }
}

/// 从 JSON value 中提取 request id。
///
/// 返回 RequestId，无法提取时使用 null 字符串。
fn extract_id(value: &serde_json::Value) -> Option<RequestId> {
    match value.get("id") {
        Some(serde_json::Value::String(s)) => Some(RequestId::String(s.clone())),
        Some(serde_json::Value::Number(n)) => n.as_i64().map(RequestId::Number),
        _ => None,
    }
}

/// 判断 JSON-RPC request id 是否为支持的 string 或 integer。
fn is_valid_request_id(value: &serde_json::Value) -> bool {
    value.as_str().is_some() || value.as_i64().is_some()
}

/// 返回 request id 的序列化字节长度。
fn request_id_bytes(value: &serde_json::Value) -> usize {
    value.as_str().map(str::len).unwrap_or_else(|| value.to_string().len())
}

/// 返回用于日志的 request id 文本。
fn format_request_id(id: &Option<RequestId>) -> String {
    id.as_ref().map(ToString::to_string).unwrap_or_else(|| "null".to_string())
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
pub fn build_error_response(id: Option<RequestId>, error: &ErrorObject) -> serde_json::Value {
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
    fn test_unknown_envelope_field_returns_invalid_request() {
        let caps = make_caps();
        let input = r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{},"extra":true}"#;
        let (_, err) = dispatch(input, &caps).unwrap_err();
        assert_eq!(err.code, -32600);
    }

    #[test]
    fn test_null_request_id_returns_invalid_request_with_null_id() {
        let caps = make_caps();
        let input = r#"{"jsonrpc":"2.0","id":null,"method":"rpc.echo","params":{}}"#;
        let (id, err) = dispatch(input, &caps).unwrap_err();
        assert_eq!(id, None);
        assert_eq!(err.code, -32600);
        assert_eq!(
            build_error_response(id, &err)["id"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn test_method_too_long_returns_payload_too_large() {
        let caps = make_caps();
        let method = "a".repeat(257);
        let input = format!(
            r#"{{"jsonrpc":"2.0","id":"1","method":"{}","params":{{}}}}"#,
            method
        );
        let (_, err) = dispatch(&input, &caps).unwrap_err();
        assert_eq!(err.code, -32003);
    }

    #[test]
    fn test_request_id_too_long_returns_payload_too_large() {
        let caps = make_caps();
        let id = "a".repeat(257);
        let input = format!(
            r#"{{"jsonrpc":"2.0","id":"{}","method":"rpc.echo","params":{{}}}}"#,
            id
        );
        let (_, err) = dispatch(&input, &caps).unwrap_err();
        assert_eq!(err.code, -32003);
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
        assert_eq!(id, Some(RequestId::String("my-id".to_string())));
    }

    #[test]
    fn test_build_error_response_format() {
        let id = RequestId::String("req-1".to_string());
        let err = ErrorObject::method_not_found("test");
        let resp = build_error_response(Some(id), &err);
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
