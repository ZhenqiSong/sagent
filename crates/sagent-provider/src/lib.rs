//! Provider-neutral 模型调用契约。
//!
//! 本 crate 只定义 Provider 请求、流式事件和错误边界，不负责 SQLite、Turn 状态
//! 或 RuntimeEvent。具体 Provider（例如 OpenAI-compatible SSE）实现这一契约，
//! 并通过 [`ModelProvider`] 把结果交给 `sagent-runtime`。

mod error;
mod provider;
mod types;

/// 可复用的测试 Provider 和本地 SSE Server；不访问真实模型服务。
pub mod mock;
/// OpenAI-compatible HTTP/SSE Provider adapter。
pub mod openai;
pub mod openai_event;
pub mod sse;

pub use error::ProviderError;
pub use openai::OpenAiCompatibleProvider;
pub use openai_event::{OpenAiStreamParser, parse_openai_frame};
pub use provider::{ModelProvider, ProviderEventSink};
pub use sse::{MAX_SSE_FRAME_SIZE, SseDecoder, SseEvent, SseFrame};
pub use types::{
    ProviderEvent, ProviderFinish, ProviderMessage, ProviderRequest, ProviderRole,
    ProviderToolCall, StopReason, TokenUsage,
};
