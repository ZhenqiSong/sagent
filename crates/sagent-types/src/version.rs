//! 协议版本模块。
//!
//! 定义协议名称、版本号和 capability 声明。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 协议版本类型

use serde::{Deserialize, Serialize};

/// 协议版本信息。
///
/// 协议版本与 Runtime 版本分离，独立演进。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolVersion {
    /// 协议族标识（固定为 "sagent.rpc"）
    pub protocol: String,
    /// 协议主版本号（不兼容变化递增）
    pub version: u32,
    /// Runtime 发布版本（仅供展示，不用于协议协商）
    pub runtime_version: String,
    /// 支持的 capability 列表
    pub features: Vec<String>,
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        Self {
            protocol: "sagent.rpc".to_string(),
            version: 1,
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
            features: vec![
                "rpc.echo".to_string(),
                "protocol.describe".to_string(),
                "health.get".to_string(),
            ],
        }
    }
}
