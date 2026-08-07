//! 通用 envelope 模块。
//!
//! 定义消息的通用外层包装结构。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 通用 envelope 类型

use serde::{Deserialize, Serialize};

/// 通用消息 envelope。
///
/// 为消息提供统一的元数据包装。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// 协议版本标识
    pub protocol: String,
    /// 协议主版本号
    pub version: u32,
    /// 消息体
    pub data: T,
}
