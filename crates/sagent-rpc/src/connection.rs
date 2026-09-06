//! 单条 RPC 连接的状态与状态化请求分发。

use sagent_protocol::{
    ConnectionAccess, DispatchService, JsonRpcRequest, JsonRpcResponse, dispatch_with_access,
};
use sagent_types::ClientCapabilities;
use serde_json::Value;

/// 一条 NDJSON 连接的生命周期状态。
///
/// `sagent-rpc` 进程可以依次接收多个请求，但这些请求属于同一条 stdin/stdout
/// 连接；因此 capability 必须挂在这个对象上，而不能放在全局变量或单次请求中。
#[derive(Debug, Default)]
pub struct ConnectionState {
    access: ConnectionAccess,
}

impl ConnectionState {
    /// 创建一个尚未完成 `client.hello` 的连接状态。
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前连接已经协商的客户端能力。
    #[allow(dead_code)] // 后续 Runtime 接线会用它把 client capability 传给 SessionActor。
    pub fn client(&self) -> Option<&ClientCapabilities> {
        self.access.client()
    }

    /// 将请求交给协议层，并复用当前连接的 capability 快照。
    ///
    /// `client.hello` 只有在版本协商成功后才会写入 `access`；其它交互方法在进入
    /// 具体 handler 前会经过握手和 approval capability gate。通知仍保持无响应语义。
    pub fn dispatch<S: DispatchService>(
        &mut self,
        request: JsonRpcRequest,
        service: &S,
    ) -> Option<JsonRpcResponse<Value>> {
        dispatch_with_access(request, service, &mut self.access)
    }
}

#[cfg(test)]
mod tests {
    use sagent_protocol::{
        GatewayPingResult, JsonRpcRequest, RequestId, SessionCreateParams, SessionCreateResult,
        SessionCreateService, SessionListParams, SessionListResult, SessionReadService,
        SessionResumeParams, SessionResumeResult,
    };
    use sagent_types::{ClientId, ClientSurface};
    use serde_json::json;

    use super::ConnectionState;

    struct FakeService;

    impl sagent_protocol::GatewayService for FakeService {
        fn ping(&self) -> GatewayPingResult {
            GatewayPingResult {
                ok: true,
                protocol_version: sagent_protocol::PROTOCOL_VERSION,
            }
        }
    }

    impl SessionReadService for FakeService {
        fn list_sessions(
            &self,
            _: &SessionListParams,
        ) -> Result<SessionListResult, sagent_protocol::ProtocolError> {
            Ok(SessionListResult {
                sessions: vec![],
                limit: 50,
                offset: 0,
            })
        }

        fn resume_session(
            &self,
            _: &SessionResumeParams,
        ) -> Result<SessionResumeResult, sagent_protocol::ProtocolError> {
            Err(sagent_protocol::ProtocolError::SessionNotFound(
                "missing".to_owned(),
            ))
        }
    }

    impl SessionCreateService for FakeService {
        fn create_session(
            &self,
            _: &SessionCreateParams,
        ) -> Result<SessionCreateResult, sagent_protocol::ProtocolError> {
            Err(sagent_protocol::ProtocolError::Internal(
                "create is not used by this fake".to_owned(),
            ))
        }
    }

    fn request(id: i64, method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(RequestId::Number(id.into())),
            method: method.to_owned(),
            params: Some(params),
        }
    }

    fn hello(id: i64, interactive_approval: bool) -> JsonRpcRequest {
        request(
            id,
            "client.hello",
            json!({
                "protocol_version": sagent_protocol::PROTOCOL_VERSION,
                "client_id": ClientId::new(),
                "surface": ClientSurface::Tui,
                "capabilities": {
                    "interactive_approval": interactive_approval,
                    "supports_stream_edits": false
                }
            }),
        )
    }

    #[test]
    fn successful_hello_is_saved_on_the_same_connection() {
        let mut state = ConnectionState::new();
        assert!(state.client().is_none());

        let response = state
            .dispatch(hello(1, true), &FakeService)
            .expect("带 id 的 hello 应返回响应");

        assert!(response.error.is_none());
        assert!(state.client().is_some());
        assert!(
            state
                .client()
                .expect("hello 后应有 client")
                .interactive_approval
        );
    }

    #[test]
    fn failed_hello_does_not_change_connection_state() {
        let mut state = ConnectionState::new();
        let mut invalid_hello = hello(1, true);
        invalid_hello.params.as_mut().expect("hello 应有参数")["protocol_version"] = json!(999);

        let response = state
            .dispatch(invalid_hello, &FakeService)
            .expect("错误 hello 也应返回错误响应");

        assert_eq!(
            response.error.expect("应返回版本错误").code,
            sagent_protocol::UNSUPPORTED_PROTOCOL_VERSION
        );
        assert!(state.client().is_none());

        let response = state
            .dispatch(
                request(
                    2,
                    "prompt.submit",
                    json!({
                        "session_id": "missing",
                        "text": "hello"
                    }),
                ),
                &FakeService,
            )
            .expect("交互请求应返回握手错误");
        assert_eq!(
            response.error.expect("应有握手错误").code,
            sagent_protocol::HANDSHAKE_REQUIRED
        );
    }

    #[test]
    fn read_only_methods_remain_available_before_hello() {
        let mut state = ConnectionState::new();
        let response = state
            .dispatch(request(1, "gateway.ping", json!({})), &FakeService)
            .expect("只读 ping 应返回响应");

        assert_eq!(response.result.expect("应有结果")["ok"], json!(true));
    }

    #[test]
    fn session_create_requires_a_successful_hello() {
        let mut state = ConnectionState::new();
        let response = state
            .dispatch(request(1, "session.create", json!({})), &FakeService)
            .expect("带 id 的交互请求应返回错误响应");

        assert_eq!(
            response.error.expect("应有握手错误").code,
            sagent_protocol::HANDSHAKE_REQUIRED
        );
    }

    #[test]
    fn approval_requires_the_connection_capability() {
        let mut state = ConnectionState::new();
        state.dispatch(hello(1, false), &FakeService);

        let response = state
            .dispatch(
                request(
                    2,
                    "approval.respond",
                    json!({
                        "session_id": "missing",
                        "turn_id": "missing",
                        "approval_id": "missing",
                        "decision": "deny"
                    }),
                ),
                &FakeService,
            )
            .expect("审批请求应返回 capability 错误");

        assert_eq!(
            response.error.expect("应有 capability 错误").code,
            sagent_protocol::CAPABILITY_NOT_GRANTED
        );
    }

    #[test]
    fn connection_states_are_isolated() {
        let mut approved = ConnectionState::new();
        let mut unapproved = ConnectionState::new();
        approved.dispatch(hello(1, true), &FakeService);

        assert!(approved.client().is_some());
        assert!(unapproved.client().is_none());

        let response = unapproved
            .dispatch(
                request(
                    2,
                    "approval.respond",
                    json!({
                        "session_id": "missing",
                        "turn_id": "missing",
                        "approval_id": "missing",
                        "decision": "deny"
                    }),
                ),
                &FakeService,
            )
            .expect("未握手连接应返回握手错误");
        assert_eq!(
            response.error.expect("应有握手错误").code,
            sagent_protocol::HANDSHAKE_REQUIRED
        );
    }
}
