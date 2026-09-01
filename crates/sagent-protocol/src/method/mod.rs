//! 各 JSON-RPC 方法的参数和结果 DTO。
//!
//! 按方法域拆分文件，避免协议方法增加后由单个文件承载所有类型。

pub mod gateway;
pub mod session;

pub use gateway::{GatewayPingParams, GatewayPingResult, PROTOCOL_VERSION, ProtocolFeatures};
pub use session::{
    SessionDetailDto, SessionListParams, SessionListResult, SessionMessageDto, SessionResumeParams,
    SessionResumeResult, SessionSummaryDto,
};
