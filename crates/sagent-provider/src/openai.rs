//! OpenAI-compatible Chat Completions HTTP/SSE adapter。
//!
//! 该模块只负责把 Provider DTO 转换成 HTTP 请求，并把上游的 SSE 字节流交给
//! [`crate::OpenAiStreamParser`]。会话、Turn 和消息的持久化不属于这里的职责。

use crate::{
    ModelProvider, OpenAiStreamParser, ProviderError, ProviderEvent, ProviderEventSink,
    ProviderFinish, ProviderRequest, ProviderRole, StopReason, TokenUsage,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url, header};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

/// OpenAI-compatible Chat Completions Provider。
///
/// API key 只保存在 adapter 内部，不会出现在 `ProviderRequest`、错误文本或调试输出中。
pub struct OpenAiCompatibleProvider {
    client: Client,
    endpoint: Url,
    api_key: String,
}

impl OpenAiCompatibleProvider {
    /// 使用已经解析好的 endpoint 创建 Provider。
    pub fn new(endpoint: Url, api_key: impl Into<String>) -> Result<Self, ProviderError> {
        if endpoint.scheme() != "http" && endpoint.scheme() != "https" {
            return Err(ProviderError::Configuration(
                "Provider endpoint 必须使用 http 或 https".into(),
            ));
        }
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(ProviderError::Configuration(
                "Provider API key 不能为空".into(),
            ));
        }
        let client = Client::builder()
            .build()
            .map_err(|_| ProviderError::Configuration("无法创建 HTTP client".into()))?;
        Ok(Self {
            client,
            endpoint,
            api_key,
        })
    }

    /// 从字符串 endpoint 创建 Provider，便于 Profile 配置层调用。
    pub fn from_endpoint(
        endpoint: &str,
        api_key: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        let endpoint = Url::parse(endpoint)
            .map_err(|_| ProviderError::Configuration("Provider endpoint 不是有效 URL".into()))?;
        Self::new(endpoint, api_key)
    }

    /// 返回 endpoint，不返回 API key。
    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }
}

#[derive(Debug, Serialize)]
struct OpenAiRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAiMessage<'a>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

#[derive(Debug, Serialize)]
struct OpenAiMessage<'a> {
    role: &'static str,
    content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

fn build_request(request: &ProviderRequest) -> OpenAiRequest<'_> {
    OpenAiRequest {
        model: &request.model,
        messages: request
            .messages
            .iter()
            .map(|message| OpenAiMessage {
                role: role_name(message.role),
                content: &message.content,
                tool_call_id: message.tool_call_id.as_deref(),
            })
            .collect(),
        stream: request.stream,
        temperature: request.temperature,
        stream_options: request.stream.then_some(StreamOptions {
            include_usage: true,
        }),
    }
}

fn role_name(role: ProviderRole) -> &'static str {
    match role {
        ProviderRole::System => "system",
        ProviderRole::User => "user",
        ProviderRole::Assistant => "assistant",
        ProviderRole::Tool => "tool",
    }
}

fn transport_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Transport("请求超时".into())
    } else if error.is_connect() {
        ProviderError::Transport("无法连接 Provider".into())
    } else {
        ProviderError::Transport("HTTP 请求失败".into())
    }
}

fn status_error(response: &reqwest::Response) -> ProviderError {
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        ProviderError::Authentication
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        ProviderError::RateLimited {
            retry_after_seconds: response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok()),
        }
    } else if status.is_server_error() {
        ProviderError::RemoteServer {
            status: status.as_u16(),
        }
    } else {
        ProviderError::Configuration(format!("Provider 返回 HTTP {}", status.as_u16()))
    }
}

fn observe_event(
    event: &ProviderEvent,
    reason: &mut Option<StopReason>,
    usage: &mut Option<TokenUsage>,
) {
    match event {
        ProviderEvent::Finished { reason: value } => *reason = Some(*value),
        ProviderEvent::Usage { usage: value } => *usage = Some(*value),
        ProviderEvent::TextDelta { .. } | ProviderEvent::ToolCallDelta { .. } => {}
    }
}

async fn emit_events(
    events: Vec<ProviderEvent>,
    sink: &mut dyn ProviderEventSink,
    reason: &mut Option<StopReason>,
    usage: &mut Option<TokenUsage>,
) -> Result<(), ProviderError> {
    for event in events {
        observe_event(&event, reason, usage);
        sink.emit(event).await?;
    }
    Ok(())
}

#[async_trait]
impl ModelProvider for OpenAiCompatibleProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        sink: &mut dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ProviderFinish, ProviderError> {
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = self.client
                .post(self.endpoint.clone())
                .bearer_auth(&self.api_key)
                .header(header::ACCEPT, "text/event-stream")
                .json(&build_request(&request))
                .send() => result.map_err(transport_error)?,
        };

        if !response.status().is_success() {
            return Err(status_error(&response));
        }

        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !content_type
            .split(';')
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
        {
            return Err(ProviderError::Protocol(
                "Provider 响应不是 text/event-stream".into(),
            ));
        }

        let provider_request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let mut stream = response.bytes_stream();
        let mut parser = OpenAiStreamParser::new();
        let mut reason = None;
        let mut usage = None;

        loop {
            let item = tokio::select! {
                _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                item = stream.next() => item,
            };
            let Some(item) = item else { break };
            let bytes = item.map_err(transport_error)?;
            let events = parser.push(&bytes)?;
            emit_events(events, sink, &mut reason, &mut usage).await?;
        }

        let events = parser.finish()?;
        emit_events(events, sink, &mut reason, &mut usage).await?;
        Ok(ProviderFinish {
            reason: reason.unwrap_or(StopReason::Unknown),
            usage,
            provider_request_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenAiCompatibleProvider, build_request, role_name};
    use crate::{ProviderMessage, ProviderRequest, ProviderRole};
    use sagent_types::{SessionId, TurnId};

    #[test]
    fn builds_openai_request_without_secret() {
        let request = ProviderRequest {
            session_id: SessionId::new("s"),
            turn_id: TurnId::new(),
            request_id: "r".into(),
            model: "gpt-test".into(),
            messages: vec![ProviderMessage {
                role: ProviderRole::Tool,
                content: "ok".into(),
                tool_call_id: Some("call-1".into()),
            }],
            temperature: Some(0.2),
            stream: true,
        };
        let json = serde_json::to_value(build_request(&request)).unwrap();
        assert_eq!(json["messages"][0]["role"], "tool");
        assert_eq!(json["messages"][0]["tool_call_id"], "call-1");
        assert_eq!(json["stream_options"]["include_usage"], true);
        assert!(!serde_json::to_string(&json).unwrap().contains("api_key"));
    }

    #[test]
    fn validates_endpoint_and_key() {
        assert!(OpenAiCompatibleProvider::from_endpoint("not-a-url", "key").is_err());
        assert!(OpenAiCompatibleProvider::from_endpoint("http://localhost", " ").is_err());
        assert_eq!(role_name(ProviderRole::Assistant), "assistant");
    }
}
