//! 工具类型模块。
//!
//! 定义 ToolCall（工具调用实例）和 ToolDefinition（工具 schema 定义）。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 工具类型定义

use serde::{Deserialize, Serialize};

use crate::ids::ToolCallId;

/// 工具调用实例。
///
/// 表示模型发起的一次工具调用请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// 工具调用唯一标识
    pub id: ToolCallId,
    /// 工具名称
    pub name: String,
    /// 工具参数（JSON object）
    pub arguments: serde_json::Map<String, serde_json::Value>,
}

/// 工具定义。
///
/// 描述一个工具的公开 schema，暴露给模型用于 function calling。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// 工具名称
    pub name: String,
    /// 工具描述
    pub description: String,
    /// 工具输入参数 JSON Schema
    pub input_schema: serde_json::Value,
}
