//! sagent-api — JSON-RPC 协议层。
//!
//! 本 crate 提供 JSON-RPC 2.0 的类型定义、错误码、schema 和 stdio transport。
//! Phase 0 只承载协议边界，不实现 Session 或 Agent 业务。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 JSON-RPC 协议层定义

/// JSON-RPC request 类型
pub mod request;

/// JSON-RPC response 类型
pub mod response;

/// JSON-RPC 错误码和错误对象
pub mod error;

/// Event envelope 和事件类型
pub mod event;

/// Schema 生成和校验
pub mod schema;

/// 日志初始化模块（tracing，stderr 输出）
pub mod logging;

/// 路径解析模块（SAGENT_HOME）
pub mod paths;

/// Session RPC 参数类型。
pub mod session;
