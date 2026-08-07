//! 协议版本模块。
//!
//! 定义协议名称、版本号和 capability 声明。
//! 客户端只能调用服务端声明的能力。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 协议版本类型
//! @change   2025-08-07 增强：添加 Capabilities 类型，提供方法注册和校验机制

use serde::{Deserialize, Serialize};

/// Phase 0 已注册的方法列表（权威来源）。
///
/// 此常量是 `protocol.describe` 返回的 feature 列表的唯一定义来源。
/// 新增方法必须在此注册。
pub const PHASE0_METHODS: &[&str] = &["rpc.echo", "protocol.describe", "health.get"];

/// 协议版本信息。
///
/// 协议版本与 Runtime 版本分离，独立演进。
/// `features` 列表必须与实际注册的方法一致。
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
            features: PHASE0_METHODS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// 能力声明集合。
///
/// 封装方法注册、查询和校验逻辑。
/// 确保 `protocol.describe` 返回的 feature 列表与实际注册的方法一致。
///
/// # 示例
///
/// ```rust
/// use sagent_types::version::Capabilities;
///
/// let caps = Capabilities::phase0_defaults();
/// assert!(caps.supports("rpc.echo"));
/// assert!(caps.supports("protocol.describe"));
/// assert!(!caps.supports("session.create"));
/// ```
#[derive(Debug, Clone)]
pub struct Capabilities {
    methods: Vec<String>,
}

impl Capabilities {
    /// 使用 Phase 0 默认方法集合创建 Capabilities。
    pub fn phase0_defaults() -> Self {
        Self {
            methods: PHASE0_METHODS.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// 从方法名列表创建 Capabilities。
    pub fn new(methods: Vec<String>) -> Self {
        Self { methods }
    }

    /// 返回是否支持指定的方法（capability）。
    pub fn supports(&self, method: &str) -> bool {
        self.methods.iter().any(|m| m == method)
    }

    /// 返回已注册的方法数量。
    pub fn len(&self) -> usize {
        self.methods.len()
    }

    /// 返回是否没有注册任何方法。
    pub fn is_empty(&self) -> bool {
        self.methods.is_empty()
    }

    /// 返回已注册方法的迭代器。
    pub fn methods(&self) -> impl Iterator<Item = &str> {
        self.methods.iter().map(|s| s.as_str())
    }

    /// 返回 feature 名称列表（用于 protocol.describe 响应）。
    pub fn feature_names(&self) -> Vec<String> {
        self.methods.clone()
    }

    /// 验证请求的方法是否在 capability 列表中。
    ///
    /// 如果方法不支持，返回 false；如果支持，返回 true。
    pub fn validate_method(&self, method: &str) -> bool {
        self.supports(method)
    }
}

impl Default for Capabilities {
    fn default() -> Self {
        Self::phase0_defaults()
    }
}
