//! `gateway.*` 方法的协议类型。

use serde::{Deserialize, Serialize};

/// 当前 JSON-RPC 协议版本。
pub const PROTOCOL_VERSION: u32 = 1;

/// `gateway.ready` 的能力载荷。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProtocolFeatures {
    /// 客户端可据此选择兼容的协议处理路径。
    pub protocol_version: u32,
    /// 当前服务支持的 JSON-RPC 请求方法。
    pub features: Vec<String>,
}

impl ProtocolFeatures {
    /// 返回第三阶段服务启动后宣告的固定能力集合。
    pub fn phase_three() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            features: vec![
                "gateway.ping".to_owned(),
                "session.list".to_owned(),
                "session.resume".to_owned(),
            ],
        }
    }
}

/// `gateway.ping` 参数；空对象或省略参数都映射到该类型。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayPingParams {}

/// `gateway.ping` 的连通性确认结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayPingResult {
    /// 永远为 `true`，用于区分成功响应和 transport 失败。
    pub ok: bool,
    /// 服务端实现的协议版本。
    pub protocol_version: u32,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{GatewayPingParams, PROTOCOL_VERSION, ProtocolFeatures};

    #[test]
    fn ready_features_advertise_the_first_read_only_methods() {
        let features = ProtocolFeatures::phase_three();
        assert_eq!(features.protocol_version, PROTOCOL_VERSION);
        assert_eq!(
            features.features,
            vec!["gateway.ping", "session.list", "session.resume"]
        );
        assert_eq!(
            serde_json::to_value(features).expect("能力集合应能序列化"),
            json!({
                "protocol_version": 1,
                "features": ["gateway.ping", "session.list", "session.resume"]
            })
        );
    }

    #[test]
    fn ping_accepts_an_empty_object() {
        let params: GatewayPingParams =
            serde_json::from_value(json!({})).expect("ping 空参数应能反序列化");

        assert_eq!(params, GatewayPingParams::default());
    }
}
