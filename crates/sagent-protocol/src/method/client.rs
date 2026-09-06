//! `client.*` 方法的握手协议类型与能力协商。
//!
//! 握手的输入是连接自身的事实，不能由单次 `prompt.submit` 等交互请求携带。这样
//! approval 是否可用始终由连接的 surface/capability 决定，而不是 RPC 进程环境或
//! 任意业务参数。

use sagent_types::{ClientCapabilities, ClientId, ClientSurface};
use serde::{Deserialize, Serialize};

use crate::method::registry::registered_features;
use crate::{PROTOCOL_VERSION, ProtocolError};

/// `client.hello` 内嵌的客户端能力声明。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientHelloCapabilities {
    /// 客户端是否能展示 approval 请求并提交用户决定。
    pub interactive_approval: bool,
    /// 客户端是否支持在原消息位置实时编辑流式文本。
    pub supports_stream_edits: bool,
}

/// `client.hello` 的请求参数。
///
/// `protocol_version` 位于顶层，避免把连接协议版本混入具体 UI capability；通过
/// [`Self::client_capabilities`] 可转换为 Runtime 使用的领域类型。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientHelloParams {
    /// 客户端实现的协议版本。
    pub protocol_version: u32,
    /// 此连接实例的稳定标识。
    pub client_id: ClientId,
    /// 客户端呈现表面。
    pub surface: ClientSurface,
    /// 与 UI 能力有关的声明。
    pub capabilities: ClientHelloCapabilities,
}

impl ClientHelloParams {
    /// 转换为 Actor/Runtime 使用的连接能力快照。
    pub fn client_capabilities(&self) -> ClientCapabilities {
        ClientCapabilities {
            client_id: self.client_id,
            surface: self.surface,
            interactive_approval: self.capabilities.interactive_approval,
            supports_stream_edits: self.capabilities.supports_stream_edits,
            protocol_version: self.protocol_version,
        }
    }
}

/// 当前连接对并发 prompt 的公开策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionBusyPolicy {
    /// 同一 Session 有活跃 Turn 时拒绝新的 submit；不隐式排队。
    Reject,
}

/// 协商成功后由服务端明确交给客户端的 session 规则。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPolicy {
    /// 交互方法是否要求该连接先成功执行 `client.hello`。
    pub requires_hello_for_interactive: bool,
    /// 同一 Session 已忙时的处理规则。
    pub busy_policy: SessionBusyPolicy,
}

/// `client.hello` 成功响应。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHelloResult {
    /// 与客户端协商后使用的协议版本。
    pub protocol_version: u32,
    /// 该连接可调用的第四阶段交互方法，按稳定顺序返回。
    pub features: Vec<String>,
    /// 服务端接受的客户端能力声明；方法仍只以 `features` 为准。
    pub capabilities: ClientHelloCapabilities,
    /// 明确的会话并发与握手规则。
    pub session_policy: SessionPolicy,
}

/// 校验协议版本并返回当前真实注册的方法。
///
/// 函数没有 I/O 和连接状态；stdio/WebSocket transport 在握手成功后保存
/// `ClientCapabilities`，再用同一份规则拦截后续交互请求。
pub fn negotiate_hello(params: &ClientHelloParams) -> Result<ClientHelloResult, ProtocolError> {
    if params.protocol_version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedProtocolVersion {
            requested: params.protocol_version,
            supported: PROTOCOL_VERSION,
        });
    }

    Ok(ClientHelloResult {
        protocol_version: PROTOCOL_VERSION,
        features: registered_features(),
        capabilities: params.capabilities.clone(),
        session_policy: SessionPolicy {
            requires_hello_for_interactive: true,
            busy_policy: SessionBusyPolicy::Reject,
        },
    })
}

#[cfg(test)]
mod tests {
    use sagent_types::{ClientId, ClientSurface};
    use serde_json::json;

    use super::{ClientHelloCapabilities, ClientHelloParams, SessionBusyPolicy, negotiate_hello};
    use crate::{PROTOCOL_VERSION, ProtocolError};

    fn hello(interactive_approval: bool) -> ClientHelloParams {
        ClientHelloParams {
            protocol_version: PROTOCOL_VERSION,
            client_id: ClientId::new(),
            surface: ClientSurface::Tui,
            capabilities: ClientHelloCapabilities {
                interactive_approval,
                supports_stream_edits: false,
            },
        }
    }

    #[test]
    fn hello_only_advertises_currently_registered_methods() {
        let result = negotiate_hello(&hello(true)).expect("相同版本应协商成功");

        assert_eq!(
            result.features,
            [
                "gateway.ping",
                "session.list",
                "session.resume",
                "client.hello",
            ]
        );
        assert!(result.capabilities.interactive_approval);
        assert!(result.session_policy.requires_hello_for_interactive);
        assert_eq!(result.session_policy.busy_policy, SessionBusyPolicy::Reject);
    }

    #[test]
    fn non_interactive_client_capability_is_reported_without_advertising_unwired_methods() {
        let result = negotiate_hello(&hello(false)).expect("相同版本应协商成功");

        assert!(!result.capabilities.interactive_approval);
        assert!(
            !result
                .features
                .iter()
                .any(|feature| feature == "approval.respond")
        );
    }

    #[test]
    fn unsupported_version_returns_requested_and_supported_versions() {
        let mut params = hello(true);
        params.protocol_version = PROTOCOL_VERSION + 1;

        assert!(matches!(
            negotiate_hello(&params),
            Err(ProtocolError::UnsupportedProtocolVersion {
                requested,
                supported
            }) if requested == PROTOCOL_VERSION + 1 && supported == PROTOCOL_VERSION
        ));
    }

    #[test]
    fn hello_wire_format_rejects_unknown_capability_fields() {
        let value = json!({
            "protocol_version": PROTOCOL_VERSION,
            "client_id": ClientId::new(),
            "surface": "tui",
            "capabilities": {
                "interactive_approval": true,
                "supports_stream_edits": false,
                "untrusted_override": true
            }
        });

        assert!(serde_json::from_value::<ClientHelloParams>(value).is_err());
    }

    #[test]
    fn hello_converts_to_runtime_capabilities_without_losing_connection_identity() {
        let params = hello(true);
        let capabilities = params.client_capabilities();

        assert_eq!(capabilities.client_id, params.client_id);
        assert_eq!(capabilities.surface, ClientSurface::Tui);
        assert!(capabilities.interactive_approval);
        assert_eq!(capabilities.protocol_version, PROTOCOL_VERSION);
    }
}
