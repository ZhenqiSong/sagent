//! JSON-RPC 服务层模块。
//!
//! 按服务职责拆分实现；当前包含只读会话服务，后续可在此加入网关状态服务、
//! DTO 映射辅助模块，而无需继续扩张单个源文件。

mod session;

pub use session::{DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT, SessionReadService, SessionService};
