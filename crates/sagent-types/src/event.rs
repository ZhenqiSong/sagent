//! 事件类型模块。
//!
//! 定义 ModelEvent 和各类事件 payload。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 事件类型定义

use serde::{Deserialize, Serialize};

/// 模型事件。
///
/// 纯数据结构，不负责写 stdout、数据库或调用模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEvent {
    /// 文本增量（流式输出片段）
    MessageDelta {
        /// 增量文本
        delta: String,
    },
    /// 工具调用开始
    ToolStart {
        /// 工具名称
        name: String,
    },
    /// 工具调用完成
    ToolComplete {
        /// 工具名称
        name: String,
        /// 执行结果
        result: serde_json::Value,
    },
    /// Turn 完成
    TurnCompleted {
        /// 完成原因
        reason: String,
    },
    /// 发生错误
    Error {
        /// 错误码
        code: i32,
        /// 错误消息
        message: String,
    },
}
