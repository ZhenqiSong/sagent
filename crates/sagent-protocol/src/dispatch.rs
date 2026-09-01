//! JSON-RPC 请求的纯校验与方法分派。
//!
//! 该模块不读取配置或数据库。服务依赖通过 [`GatewayService`] 注入，便于用 fake 服务
//! 覆盖协议行为；后续 `session.*` 方法可以在同一入口扩展，而不会污染 stdio 循环。

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{
    GatewayPingParams, GatewayPingResult, JsonRpcRequest, JsonRpcResponse, ProtocolError,
    RequestId, SessionListParams, SessionReadService, SessionResumeParams,
};

/// 网关基础能力的最小服务接口。
pub trait GatewayService {
    /// 执行无副作用的连通性检查。
    fn ping(&self) -> GatewayPingResult;
}

/// 可被 JSON-RPC 入口直接分派的完整只读服务能力。
pub trait DispatchService: GatewayService + SessionReadService {}

impl<T> DispatchService for T where T: GatewayService + SessionReadService {}

/// 校验并分派一个请求。
///
/// 返回 `None` 表示通知：即使通知处理失败，也不能向 stdout 写响应。带 `id` 的请求
/// 总会得到成功或错误响应；错误的 `id` 会被原样带回。
pub fn dispatch<S: DispatchService>(
    request: JsonRpcRequest,
    service: &S,
) -> Option<JsonRpcResponse<Value>> {
    let id = request.id.clone();
    let result = dispatch_request(request, service);

    id.map(|request_id| match result {
        Ok(result) => JsonRpcResponse::success(request_id, result),
        Err(error) => JsonRpcResponse::failure(request_id, error.to_jsonrpc()),
    })
}

fn dispatch_request<S: DispatchService>(
    request: JsonRpcRequest,
    service: &S,
) -> Result<Value, ProtocolError> {
    if request.jsonrpc != "2.0" || request.method.trim().is_empty() {
        return Err(ProtocolError::InvalidRequest);
    }

    let (namespace, action) = request
        .method
        .split_once('.')
        .ok_or_else(|| ProtocolError::MethodNotFound(request.method.clone()))?;

    match namespace {
        "gateway" => dispatch_gateway(action, request.params, service),
        "session" => dispatch_session(action, request.params, service),
        _ => Err(ProtocolError::MethodNotFound(request.method)),
    }
}

/// `gateway.*` 方法的二级分发入口。
fn dispatch_gateway<S: DispatchService>(
    action: &str,
    params: Option<Value>,
    service: &S,
) -> Result<Value, ProtocolError> {
    match action {
        "ping" => {
            let _: GatewayPingParams = parse_params(params)?;
            serde_json::to_value(service.ping())
                .map_err(|error| ProtocolError::Internal(error.to_string()))
        }
        _ => Err(ProtocolError::MethodNotFound(format!("gateway.{action}"))),
    }
}

/// `session.*` 方法的二级分发入口。
fn dispatch_session<S: DispatchService>(
    action: &str,
    params: Option<Value>,
    service: &S,
) -> Result<Value, ProtocolError> {
    match action {
        "list" => {
            let params: SessionListParams = parse_params(params)?;
            serde_json::to_value(service.list_sessions(&params)?)
                .map_err(|error| ProtocolError::Internal(error.to_string()))
        }
        "resume" => {
            let params: SessionResumeParams = parse_params(params)?;
            serde_json::to_value(service.resume_session(&params)?)
                .map_err(|error| ProtocolError::Internal(error.to_string()))
        }
        _ => Err(ProtocolError::MethodNotFound(format!("session.{action}"))),
    }
}

/// 将可选的 JSON-RPC 参数统一解析为方法的强类型参数。
///
/// JSON-RPC 允许方法省略 `params`，这里将其视为空对象；但一旦提供参数，必须是对象，
/// 避免数组、字符串等值绕过方法参数契约。
fn parse_params<T: DeserializeOwned>(params: Option<Value>) -> Result<T, ProtocolError> {
    let params = params.unwrap_or_else(|| serde_json::json!({}));
    if !params.is_object() {
        return Err(ProtocolError::InvalidParams(
            "params must be an object".to_owned(),
        ));
    }
    serde_json::from_value(params).map_err(|error| ProtocolError::InvalidParams(error.to_string()))
}

/// 构造一个数字 ID 的请求，供协议层测试和简单客户端使用。
pub fn request_with_number_id(id: i64, method: impl Into<String>) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: Some(RequestId::Number(id.into())),
        method: method.into(),
        params: None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{GatewayService, dispatch, request_with_number_id};
    use crate::{
        GatewayPingResult, JsonRpcRequest, RequestId, SessionListParams, SessionListResult,
        SessionReadService, SessionResumeParams, SessionResumeResult,
        error::{INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND},
    };

    struct FakeGateway;

    impl GatewayService for FakeGateway {
        fn ping(&self) -> GatewayPingResult {
            GatewayPingResult {
                ok: true,
                protocol_version: 1,
            }
        }
    }

    impl SessionReadService for FakeGateway {
        fn list_sessions(
            &self,
            _params: &SessionListParams,
        ) -> Result<SessionListResult, crate::ProtocolError> {
            Ok(SessionListResult {
                sessions: vec![],
                limit: 50,
                offset: 0,
            })
        }

        fn resume_session(
            &self,
            _params: &SessionResumeParams,
        ) -> Result<SessionResumeResult, crate::ProtocolError> {
            Err(crate::ProtocolError::SessionNotFound("missing".to_owned()))
        }
    }

    #[test]
    fn ping_returns_a_result_with_the_original_id() {
        let request = request_with_number_id(7, "gateway.ping");
        let response = dispatch(request, &FakeGateway).expect("带 id 的请求应返回响应");

        assert_eq!(response.id, RequestId::Number(7.into()));
        assert_eq!(response.error, None);
        assert_eq!(
            response.result,
            Some(json!({"ok": true, "protocol_version": 1}))
        );
    }

    #[test]
    fn omitted_ping_params_are_accepted() {
        let response = dispatch(request_with_number_id(1, "gateway.ping"), &FakeGateway)
            .expect("省略参数的 ping 应返回响应");

        assert!(response.result.is_some());
    }

    #[test]
    fn invalid_jsonrpc_version_is_rejected() {
        let request = JsonRpcRequest {
            jsonrpc: "1.0".to_owned(),
            id: Some(RequestId::Number(2.into())),
            method: "gateway.ping".to_owned(),
            params: None,
        };
        let response = dispatch(request, &FakeGateway).expect("错误请求仍应返回错误响应");

        assert_eq!(response.error.expect("应有错误").code, INVALID_REQUEST);
    }

    #[test]
    fn empty_method_is_rejected() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(RequestId::Number(3.into())),
            method: "  ".to_owned(),
            params: None,
        };
        let response = dispatch(request, &FakeGateway).expect("错误请求仍应返回错误响应");

        assert_eq!(response.error.expect("应有错误").code, INVALID_REQUEST);
    }

    #[test]
    fn unknown_method_is_rejected_with_method_data() {
        let response = dispatch(request_with_number_id(4, "session.unknown"), &FakeGateway)
            .expect("未知方法应返回错误响应");
        let error = response.error.expect("应有错误");

        assert_eq!(error.code, METHOD_NOT_FOUND);
        assert_eq!(error.data, Some(json!({"method": "session.unknown"})));
    }

    #[test]
    fn ping_array_params_are_invalid() {
        let mut request = request_with_number_id(5, "gateway.ping");
        request.params = Some(json!([]));
        let response = dispatch(request, &FakeGateway).expect("参数错误应返回错误响应");

        assert_eq!(response.error.expect("应有错误").code, INVALID_PARAMS);
    }

    #[test]
    fn notification_never_produces_a_response() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: None,
            method: "gateway.ping".to_owned(),
            params: Some(json!({})),
        };

        assert!(dispatch(request, &FakeGateway).is_none());
    }

    #[test]
    fn error_response_has_no_result_field() {
        let response = dispatch(request_with_number_id(6, "missing"), &FakeGateway)
            .expect("未知方法应返回响应");
        let value = serde_json::to_value(response).expect("响应应能序列化");

        assert!(value.get("result").is_none());
        assert!(value.get("error").is_some());
        assert_eq!(value.get("id"), Some(&Value::from(6)));
    }
}
