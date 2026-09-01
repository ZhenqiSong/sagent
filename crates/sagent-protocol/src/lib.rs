//! Sagent 本地 JSON-RPC 协议的稳定数据结构。
//!
//! 本 crate 只描述传输边界，不依赖配置、SQLite 或领域对象。这样未来的 stdio、
//! WebSocket 和 TUI 可以共享同一份请求、响应与方法 DTO。

pub mod dispatch;
pub mod envelope;
pub mod error;
pub mod method;

pub use dispatch::{GatewayService, dispatch, request_with_number_id};
pub use envelope::{
    EventParams, JsonRpcError, JsonRpcEvent, JsonRpcRequest, JsonRpcResponse, RequestId,
};
pub use error::{
    INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR,
    SESSION_NOT_FOUND, STORE_UNAVAILABLE, ProtocolError,
};
pub use method::{
    GatewayPingParams, GatewayPingResult, PROTOCOL_VERSION, ProtocolFeatures, SessionDetailDto,
    SessionListParams, SessionListResult, SessionMessageDto, SessionResumeParams,
    SessionResumeResult, SessionSummaryDto,
};
