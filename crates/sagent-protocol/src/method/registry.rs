//! 已公开 JSON-RPC 方法的唯一注册表与纯访问规则。
//!
//! `gateway.ready`、`client.hello` 与后续连接 dispatcher 都从这里读取方法事实，
//! 避免协议公告了尚未注册、调用后却得到 `method not found` 的 feature。

use sagent_types::ClientCapabilities;

use crate::ProtocolError;

/// 一个已公开方法需要的最小连接访问级别。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MethodAccess {
    /// 不修改 Runtime 的旧只读方法和握手本身。
    Public,
    /// 交互方法；需要当前连接已成功 hello。
    HelloRequired,
    /// 需要 hello，且客户端能展示并响应审批卡片。
    InteractiveApprovalRequired,
}

/// 一个真实可分发的协议方法。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MethodSpec {
    /// 线上 JSON-RPC 方法名。
    pub name: &'static str,
    /// 调用该方法所需的连接状态。
    pub access: MethodAccess,
}

/// 当前 binary 已实际注册的方法。后续接通交互 handler 时，必须在同一改动中把
/// 对应项加入这里；ready/hello 才会自动公告它。
const REGISTERED_METHODS: &[MethodSpec] = &[
    MethodSpec {
        name: "gateway.ping",
        access: MethodAccess::Public,
    },
    MethodSpec {
        name: "session.list",
        access: MethodAccess::Public,
    },
    MethodSpec {
        name: "session.resume",
        access: MethodAccess::Public,
    },
    MethodSpec {
        name: "client.hello",
        access: MethodAccess::Public,
    },
];

/// 未来交互方法的访问规则先固定；它们在注册前绝不进入 feature list。
pub fn planned_method_access(method: &str) -> Option<MethodAccess> {
    match method {
        "session.create" | "prompt.submit" | "session.interrupt" | "session.events.since" => {
            Some(MethodAccess::HelloRequired)
        }
        "approval.respond" => Some(MethodAccess::InteractiveApprovalRequired),
        _ => None,
    }
}

/// 返回当前真实可调用的方法，顺序是线上协议的一部分。
pub fn registered_features() -> Vec<String> {
    REGISTERED_METHODS
        .iter()
        .map(|spec| spec.name.to_owned())
        .collect()
}

/// 查询已经注册的方法；未注册的方法不会被 ready/hello 公告。
pub fn registered_method(method: &str) -> Option<MethodSpec> {
    REGISTERED_METHODS
        .iter()
        .copied()
        .find(|spec| spec.name == method)
}

/// 一条连接当前已协商的 capability 快照。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConnectionAccess {
    client: Option<ClientCapabilities>,
}

impl ConnectionAccess {
    /// 创建尚未握手的访问状态。
    pub fn new() -> Self {
        Self::default()
    }

    /// 保存一次成功 hello 的连接能力。
    pub fn set_client(&mut self, client: ClientCapabilities) {
        self.client = Some(client);
    }

    /// 返回当前已协商能力；后续 transport 用它保存每条连接的状态。
    pub fn client(&self) -> Option<&ClientCapabilities> {
        self.client.as_ref()
    }

    /// 验证一个交互方法是否可被当前连接调用。
    pub fn require(&self, method: &str, access: MethodAccess) -> Result<(), ProtocolError> {
        match access {
            MethodAccess::Public => Ok(()),
            MethodAccess::HelloRequired => self.require_hello(method).map(|_| ()),
            MethodAccess::InteractiveApprovalRequired => {
                let client = self.require_hello(method)?;
                if client.interactive_approval {
                    Ok(())
                } else {
                    Err(ProtocolError::CapabilityNotGranted {
                        capability: "interactive_approval".to_owned(),
                        method: method.to_owned(),
                    })
                }
            }
        }
    }

    fn require_hello(&self, method: &str) -> Result<&ClientCapabilities, ProtocolError> {
        self.client
            .as_ref()
            .ok_or_else(|| ProtocolError::HandshakeRequired {
                method: method.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use sagent_types::{ClientCapabilities, ClientId, ClientSurface};

    use super::{ConnectionAccess, MethodAccess, planned_method_access, registered_features};
    use crate::ProtocolError;

    fn client(interactive_approval: bool) -> ClientCapabilities {
        ClientCapabilities {
            client_id: ClientId::new(),
            surface: ClientSurface::Tui,
            interactive_approval,
            supports_stream_edits: false,
            protocol_version: 1,
        }
    }

    #[test]
    fn registry_only_advertises_currently_dispatchable_methods() {
        assert_eq!(
            registered_features(),
            [
                "gateway.ping",
                "session.list",
                "session.resume",
                "client.hello"
            ]
        );
    }

    #[test]
    fn hello_and_approval_rules_are_kept_outside_transport_code() {
        let mut access = ConnectionAccess::new();
        assert!(matches!(
            access.require("prompt.submit", MethodAccess::HelloRequired),
            Err(ProtocolError::HandshakeRequired { .. })
        ));
        access.set_client(client(false));
        assert!(matches!(
            access.require(
                "approval.respond",
                MethodAccess::InteractiveApprovalRequired
            ),
            Err(ProtocolError::CapabilityNotGranted { .. })
        ));
        access.set_client(client(true));
        assert!(
            access
                .require(
                    "approval.respond",
                    MethodAccess::InteractiveApprovalRequired
                )
                .is_ok()
        );
        assert_eq!(
            planned_method_access("approval.respond"),
            Some(MethodAccess::InteractiveApprovalRequired)
        );
    }
}
