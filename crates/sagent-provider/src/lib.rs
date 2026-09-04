//! Provider-neutral 模型调用契约。
//!
//! 本 crate 只定义 Provider 请求、流式事件和错误边界，不负责 SQLite、Turn 状态
//! 或 RuntimeEvent。具体 Provider（例如 OpenAI-compatible SSE）在后续模块中实现，
//! 并通过 [`ModelProvider`] 把结果交给 `sagent-runtime`。

mod error;
mod provider;
mod types;

pub use error::ProviderError;
pub use provider::{ModelProvider, ProviderEventSink};
pub use types::{
    ProviderEvent, ProviderFinish, ProviderMessage, ProviderRequest, ProviderRole, StopReason,
    TokenUsage,
};
