//! sagent-types — 零 IO 依赖，纯数据模型。
//!
//! 本 crate 是 Sagent 项目的窄腰层，定义所有核心数据类型。
//! 不依赖 Tokio、SQLite、HTTP、文件系统或 CLI。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 基础类型定义

/// ID 类型模块（SessionId、TurnId、MessageId、ToolCallId）
pub mod ids;

/// 消息类型模块（Role、ContentPart、Message）
pub mod message;

/// Session 类型模块（Session、SessionStatus）
pub mod session;

/// 工具类型模块（ToolCall、ToolDefinition）
pub mod tool;

/// 事件类型模块（ModelEvent 和事件 payload）
pub mod event;

/// 通用 envelope 模块
pub mod envelope;

/// 协议版本模块（protocol name、version、features）
pub mod version;
