//! 各 JSON-RPC 方法的参数和结果 DTO。
//!
//! 按方法域拆分文件，避免协议方法增加后由单个文件承载所有类型。

pub mod approval;
pub mod client;
pub mod config;
pub mod gateway;
pub mod prompt;
pub mod registry;
pub mod session;

pub use approval::{
    ApprovalDecisionDto, ApprovalRespondParams, ApprovalRespondResult, ApprovalRespondStatus,
};
pub use client::{
    ClientHelloCapabilities, ClientHelloParams, ClientHelloResult, SessionBusyPolicy,
    SessionPolicy, negotiate_hello,
};
pub use config::{ConfigReadParams, ConfigReadResult};
pub use gateway::{GatewayPingParams, GatewayPingResult, PROTOCOL_VERSION, ProtocolFeatures};
pub use prompt::{PromptSubmitParams, PromptSubmitResult, PromptSubmitStatus};
pub use registry::{
    ConnectionAccess, MethodAccess, MethodSpec, planned_method_access, registered_features,
    registered_method,
};
pub use session::{
    SessionCreateParams, SessionCreateResult, SessionDetailDto, SessionEventDto,
    SessionEventsSinceParams, SessionEventsSinceResult, SessionInterruptParams,
    SessionInterruptResult, SessionInterruptStatus, SessionListParams, SessionListResult,
    SessionMessageDto, SessionResumeParams, SessionResumeResult, SessionSummaryDto,
};
