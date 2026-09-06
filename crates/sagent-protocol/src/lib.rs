//! Sagent 本地 JSON-RPC 协议的稳定数据结构。
//!
//! 本 crate 只描述传输边界，不依赖配置、SQLite 或领域对象。这样未来的 stdio、
//! WebSocket 和 TUI 可以共享同一份请求、响应与方法 DTO。

pub mod dispatch;
pub mod envelope;
pub mod error;
pub mod method;
pub mod service;

pub use dispatch::{DispatchService, GatewayService, dispatch, request_with_number_id};
pub use envelope::{
    EventParams, JsonRpcError, JsonRpcEvent, JsonRpcRequest, JsonRpcResponse, RequestId,
};
pub use error::{
    CAPABILITY_NOT_GRANTED, HANDSHAKE_REQUIRED, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST,
    METHOD_NOT_FOUND, NO_ACTIVE_TURN, PARSE_ERROR, ProtocolError, RUNTIME_UNAVAILABLE,
    SESSION_BUSY, SESSION_NOT_FOUND, STORE_UNAVAILABLE, UNSUPPORTED_PROTOCOL_VERSION,
};
pub use method::{
    ApprovalDecisionDto, ApprovalRespondParams, ApprovalRespondResult, ApprovalRespondStatus,
    ClientHelloCapabilities, ClientHelloParams, ClientHelloResult, ConnectionAccess,
    GatewayPingParams, GatewayPingResult, MethodAccess, MethodSpec, PROTOCOL_VERSION,
    PromptSubmitParams, PromptSubmitResult, PromptSubmitStatus, ProtocolFeatures,
    SessionBusyPolicy, SessionCreateParams, SessionCreateResult, SessionDetailDto, SessionEventDto,
    SessionEventsSinceParams, SessionEventsSinceResult, SessionInterruptParams,
    SessionInterruptResult, SessionInterruptStatus, SessionListParams, SessionListResult,
    SessionMessageDto, SessionPolicy, SessionResumeParams, SessionResumeResult, SessionSummaryDto,
    negotiate_hello, planned_method_access, registered_features, registered_method,
};
pub use service::{DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT, SessionReadService, SessionService};
