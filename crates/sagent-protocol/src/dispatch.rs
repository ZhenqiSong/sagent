//! JSON-RPC 请求的纯校验与方法分派。
//!
//! 该模块不读取配置或数据库。服务依赖通过 [`GatewayService`] 注入，便于用 fake 服务
//! 覆盖协议行为；后续 `session.*` 方法可以在同一入口扩展，而不会污染 stdio 循环。

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{
    ApprovalRespondParams, ApprovalRespondResult, ClientHelloParams, ConnectionAccess,
    GatewayPingParams, GatewayPingResult, JsonRpcRequest, JsonRpcResponse, PromptSubmitParams,
    PromptSubmitResult, ProtocolError, RequestId, SessionCreateParams, SessionCreateService,
    SessionEventsSinceParams, SessionEventsSinceResult, SessionInterruptParams,
    SessionInterruptResult, SessionListParams, SessionReadService, SessionResumeParams,
    negotiate_hello, planned_method_access,
};

/// 网关基础能力的最小服务接口。
pub trait GatewayService {
    /// 执行无副作用的连通性检查。
    fn ping(&self) -> GatewayPingResult;
}

/// 同步协议入口的 prompt 兼容接口。
///
/// 正式 RPC transport 会在进入该同步 dispatcher 前把 `prompt.submit` 转到
/// SessionActor；这个默认实现仅让没有 Tokio runtime 的协议调用者得到明确、稳定的
/// 失败，而不是错误地把已公告的方法报告为不存在。
pub trait PromptService {
    /// 同步调用方提交 prompt 的兼容入口。
    fn submit_prompt(&self, _: &PromptSubmitParams) -> Result<PromptSubmitResult, ProtocolError> {
        Err(ProtocolError::RuntimeUnavailable(
            "prompt.submit requires the RPC runtime".to_owned(),
        ))
    }
}

/// 同步协议入口的控制方法兼容接口。
///
/// 真实 transport 会把控制请求投递到异步 Actor mailbox；默认值只用于让没有 Runtime
/// 的 protocol 调用方保留已注册方法的稳定错误，不会执行取消或审批副作用。
pub trait SessionControlService {
    /// 同步调用方中断 Turn 的兼容入口。
    fn interrupt_session(
        &self,
        _: &SessionInterruptParams,
    ) -> Result<SessionInterruptResult, ProtocolError> {
        Err(ProtocolError::RuntimeUnavailable(
            "session.interrupt requires the RPC runtime".to_owned(),
        ))
    }

    /// 同步调用方提交审批决定的兼容入口。
    fn respond_approval(
        &self,
        _: &ApprovalRespondParams,
    ) -> Result<ApprovalRespondResult, ProtocolError> {
        Err(ProtocolError::RuntimeUnavailable(
            "approval.respond requires the RPC runtime".to_owned(),
        ))
    }

    /// 同步调用方补读持久化事件的兼容入口。
    fn events_since_session(
        &self,
        _: &SessionEventsSinceParams,
    ) -> Result<SessionEventsSinceResult, ProtocolError> {
        Err(ProtocolError::RuntimeUnavailable(
            "session.events.since requires the RPC runtime".to_owned(),
        ))
    }
}

/// 可被 JSON-RPC 入口直接分派的服务能力。
///
/// 第三阶段的只读方法仍由同一 trait 提供；第四阶段仅额外加入不会启动 Actor 的
/// `session.create`，避免 transport 为单个写方法引入第二套分发入口。
pub trait DispatchService:
    GatewayService + SessionReadService + SessionCreateService + PromptService + SessionControlService
{
}

impl<T> PromptService for T where T: GatewayService + SessionReadService + SessionCreateService {}
impl<T> SessionControlService for T where
    T: GatewayService + SessionReadService + SessionCreateService
{
}

impl<T> DispatchService for T where
    T: GatewayService
        + SessionReadService
        + SessionCreateService
        + PromptService
        + SessionControlService
{
}

/// 校验并分派一个请求。
///
/// 返回 `None` 表示通知：即使通知处理失败，也不能向 stdout 写响应。带 `id` 的请求
/// 总会得到成功或错误响应；错误的 `id` 会被原样带回。
pub fn dispatch<S: DispatchService>(
    request: JsonRpcRequest,
    service: &S,
) -> Option<JsonRpcResponse<Value>> {
    dispatch_response(request, service, None)
}

/// 使用一条连接的 capability 状态分派请求。
///
/// 与无状态的 [`dispatch`] 保持分离，避免破坏第三阶段只读调用方；RPC transport
/// 应为每条输入连接创建一个 `ConnectionAccess`，并在整个 NDJSON 循环中复用它。
/// 这样 `client.hello` 的结果只影响当前连接，不会泄漏到其他连接。
pub fn dispatch_with_access<S: DispatchService>(
    request: JsonRpcRequest,
    service: &S,
    access: &mut ConnectionAccess,
) -> Option<JsonRpcResponse<Value>> {
    dispatch_response(request, service, Some(access))
}

fn dispatch_response<S: DispatchService>(
    request: JsonRpcRequest,
    service: &S,
    access: Option<&mut ConnectionAccess>,
) -> Option<JsonRpcResponse<Value>> {
    let id = request.id.clone();
    let result = dispatch_request_with_access(request, service, access);

    id.map(|request_id| match result {
        Ok(result) => JsonRpcResponse::success(request_id, result),
        Err(error) => JsonRpcResponse::failure(request_id, error.to_jsonrpc()),
    })
}

fn dispatch_request_with_access<S: DispatchService>(
    request: JsonRpcRequest,
    service: &S,
    access: Option<&mut ConnectionAccess>,
) -> Result<Value, ProtocolError> {
    if request.jsonrpc != "2.0" || request.method.trim().is_empty() {
        return Err(ProtocolError::InvalidRequest);
    }

    // 只对已注册或已规划的方法做 capability gate；未知方法仍应返回稳定的
    // `method not found`，不能因为未握手而掩盖客户端拼写错误。
    if request.method != "client.hello"
        && let Some(method_access) = crate::registered_method(&request.method)
            .map(|spec| spec.access)
            .or_else(|| planned_method_access(&request.method))
        && let Some(connection_access) = access.as_ref()
    {
        connection_access.require(&request.method, method_access)?;
    }

    let (namespace, action) = request
        .method
        .split_once('.')
        .ok_or_else(|| ProtocolError::MethodNotFound(request.method.clone()))?;

    match namespace {
        "client" => dispatch_client(action, request.params, access),
        "gateway" => dispatch_gateway(action, request.params, service),
        "session" => dispatch_session(action, request.params, service),
        "prompt" => dispatch_prompt(action, request.params, service),
        "approval" => dispatch_approval(action, request.params, service),
        _ => Err(ProtocolError::MethodNotFound(request.method)),
    }
}

/// `prompt.*` 的同步兼容分发。
///
/// 真实 stdio 路径会在 RPC 层异步执行本方法；保留此入口可确保协议公告与无 runtime
/// 的调用方一致，并把参数校验集中在同一个 DTO 上。
fn dispatch_prompt<S: DispatchService>(
    action: &str,
    params: Option<Value>,
    service: &S,
) -> Result<Value, ProtocolError> {
    match action {
        "submit" => {
            let params: PromptSubmitParams = parse_params(params)?;
            serde_json::to_value(service.submit_prompt(&params)?)
                .map_err(|error| ProtocolError::Internal(error.to_string()))
        }
        _ => Err(ProtocolError::MethodNotFound(format!("prompt.{action}"))),
    }
}

/// `client.*` 方法的二级分发入口。
///
/// hello 本身是纯协商，不读取 Store；通过带状态的入口调用时，只有协商成功的
/// capability 才会保存到当前连接。
fn dispatch_client(
    action: &str,
    params: Option<Value>,
    access: Option<&mut ConnectionAccess>,
) -> Result<Value, ProtocolError> {
    match action {
        "hello" => {
            let params: ClientHelloParams = parse_params(params)?;
            let result = negotiate_hello(&params)?;
            if let Some(access) = access {
                access.set_client(params.client_capabilities());
            }
            serde_json::to_value(result).map_err(|error| ProtocolError::Internal(error.to_string()))
        }
        _ => Err(ProtocolError::MethodNotFound(format!("client.{action}"))),
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
        "create" => {
            let params: SessionCreateParams = parse_params(params)?;
            serde_json::to_value(service.create_session(&params)?)
                .map_err(|error| ProtocolError::Internal(error.to_string()))
        }
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
        "interrupt" => {
            let params: SessionInterruptParams = parse_params(params)?;
            serde_json::to_value(service.interrupt_session(&params)?)
                .map_err(|error| ProtocolError::Internal(error.to_string()))
        }
        "events.since" => {
            let params: SessionEventsSinceParams = parse_params(params)?;
            serde_json::to_value(service.events_since_session(&params)?)
                .map_err(|error| ProtocolError::Internal(error.to_string()))
        }
        _ => Err(ProtocolError::MethodNotFound(format!("session.{action}"))),
    }
}

/// `approval.*` 的同步兼容分发；实际决策仍由 RPC Actor 入口处理。
fn dispatch_approval<S: DispatchService>(
    action: &str,
    params: Option<Value>,
    service: &S,
) -> Result<Value, ProtocolError> {
    match action {
        "respond" => {
            let params: ApprovalRespondParams = parse_params(params)?;
            serde_json::to_value(service.respond_approval(&params)?)
                .map_err(|error| ProtocolError::Internal(error.to_string()))
        }
        _ => Err(ProtocolError::MethodNotFound(format!("approval.{action}"))),
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
    use sagent_types::{ClientId, ClientSurface};

    use crate::{
        GatewayPingResult, JsonRpcRequest, RequestId, SessionCreateParams, SessionCreateResult,
        SessionCreateService, SessionListParams, SessionListResult, SessionReadService,
        SessionResumeParams, SessionResumeResult,
        error::{INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND},
        registered_features,
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

    impl SessionCreateService for FakeGateway {
        fn create_session(
            &self,
            _: &SessionCreateParams,
        ) -> Result<SessionCreateResult, crate::ProtocolError> {
            Err(crate::ProtocolError::Internal(
                "create is not used by this fake".to_owned(),
            ))
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

    #[test]
    fn hello_only_advertises_registered_features_without_touching_the_service() {
        let mut request = request_with_number_id(8, "client.hello");
        request.params = Some(json!({
            "protocol_version": 1,
            "client_id": ClientId::new(),
            "surface": ClientSurface::Tui,
            "capabilities": {
                "interactive_approval": true,
                "supports_stream_edits": false
            }
        }));

        let response = dispatch(request, &FakeGateway).expect("带 id 的请求应返回响应");
        let result = response.result.expect("hello 应协商成功");

        assert_eq!(
            result["features"],
            json!([
                "gateway.ping",
                "session.list",
                "session.resume",
                "client.hello",
                "session.create",
                "prompt.submit",
                "session.interrupt",
                "approval.respond",
                "session.events.since"
            ])
        );
        assert_eq!(result["capabilities"]["interactive_approval"], true);
        assert_eq!(result["session_policy"]["busy_policy"], "reject");
    }

    #[test]
    fn every_advertised_feature_has_a_dispatch_entry() {
        for method in registered_features() {
            let mut request = request_with_number_id(9, method.clone());
            request.params = Some(match method.as_str() {
                "gateway.ping" | "session.list" | "session.create" => json!({}),
                "prompt.submit" => json!({"session_id": "missing", "text": "hello"}),
                "session.interrupt" => json!({"session_id": "missing"}),
                "approval.respond" => json!({
                    "session_id": "missing", "turn_id": "missing",
                    "approval_id": "missing", "decision": "deny"
                }),
                "session.events.since" => json!({"session_id": "missing"}),
                "session.resume" => json!({"session_id": "missing"}),
                "client.hello" => json!({
                    "protocol_version": 1,
                    "client_id": ClientId::new(),
                    "surface": ClientSurface::Api,
                    "capabilities": {
                        "interactive_approval": false,
                        "supports_stream_edits": false
                    }
                }),
                _ => unreachable!("注册表只应包含已知方法"),
            });

            let response = dispatch(request, &FakeGateway).expect("带 id 的请求应得到响应");
            assert_ne!(
                response.error.as_ref().map(|error| error.code),
                Some(METHOD_NOT_FOUND),
                "公告的方法 {method} 必须有分发入口"
            );
        }
    }
}
