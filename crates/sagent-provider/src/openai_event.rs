//! OpenAI-compatible SSE JSON 到 ProviderEvent 的转换。

use crate::{ProviderError, ProviderEvent, SseDecoder, SseEvent, SseFrame, StopReason, TokenUsage};
use serde_json::Value;

/// 增量解析 OpenAI-compatible SSE，并在 EOF 时验证 finish 与 `[DONE]`。
#[derive(Debug, Default)]
pub struct OpenAiStreamParser {
    decoder: SseDecoder,
    saw_finish: bool,
    saw_done: bool,
}

impl OpenAiStreamParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 推入网络 chunk，返回已经解析出的 ProviderEvent。
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<ProviderEvent>, ProviderError> {
        let events = self.decoder.push(bytes)?;
        self.convert(events)
    }

    /// 处理 EOF；没有明确 finish 或 `[DONE]` 都视为不完整流。
    pub fn finish(&mut self) -> Result<Vec<ProviderEvent>, ProviderError> {
        let events = self.decoder.finish()?;
        let converted = self.convert(events)?;
        if !self.saw_finish || !self.saw_done {
            return Err(ProviderError::IncompleteStream);
        }
        Ok(converted)
    }

    fn convert(&mut self, events: Vec<SseEvent>) -> Result<Vec<ProviderEvent>, ProviderError> {
        let mut output = Vec::new();
        for event in events {
            match event {
                SseEvent::Done => self.saw_done = true,
                SseEvent::Frame(frame) => {
                    let parsed = parse_openai_frame(&frame)?;
                    if parsed
                        .iter()
                        .any(|event| matches!(event, ProviderEvent::Finished { .. }))
                    {
                        self.saw_finish = true;
                    }
                    output.extend(parsed);
                }
            }
        }
        Ok(output)
    }
}

/// 将一个 OpenAI-compatible SSE data frame 转换为中性 ProviderEvent。
pub fn parse_openai_frame(frame: &SseFrame) -> Result<Vec<ProviderEvent>, ProviderError> {
    let value: Value = serde_json::from_str(&frame.data)
        .map_err(|error| ProviderError::Protocol(format!("OpenAI JSON 解析失败：{error}")))?;
    let mut events = Vec::new();

    if let Some(choices) = value.get("choices").and_then(Value::as_array) {
        for (index, choice) in choices.iter().enumerate() {
            if let Some(content) = choice
                .get("delta")
                .and_then(|delta| delta.get("content"))
                .and_then(Value::as_str)
                .filter(|content| !content.is_empty())
            {
                events.push(ProviderEvent::TextDelta {
                    text: content.to_owned(),
                });
            }

            if let Some(tool_calls) = choice
                .get("delta")
                .and_then(|delta| delta.get("tool_calls"))
                .and_then(Value::as_array)
            {
                for tool_call in tool_calls {
                    let call_index = tool_call
                        .get("index")
                        .and_then(Value::as_u64)
                        .unwrap_or(index as u64);
                    let call_id = tool_call
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| format!("index-{call_index}"));
                    let function = tool_call.get("function");
                    events.push(ProviderEvent::ToolCallDelta {
                        call_id,
                        name: function
                            .and_then(|value| value.get("name"))
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned),
                        arguments_delta: function
                            .and_then(|value| value.get("arguments"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    });
                }
            }

            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                events.push(ProviderEvent::Finished {
                    reason: stop_reason(reason),
                });
            }
        }
    }

    // OpenAI-compatible 服务通常会在普通流式 chunk 中返回 `usage: null`，
    // 只有最终 usage chunk 才提供完整统计。DeepSeek 也遵循这一约定；
    // null 不是协议错误，应等待后续包含实际字段的 usage 对象。
    if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
        events.push(ProviderEvent::Usage {
            usage: parse_usage(usage)?,
        });
    }
    Ok(events)
}

fn parse_usage(value: &Value) -> Result<TokenUsage, ProviderError> {
    let number = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_u64)
            .ok_or_else(|| ProviderError::Protocol(format!("usage 缺少有效字段：{name}")))
    };
    Ok(TokenUsage {
        prompt_tokens: number("prompt_tokens")?,
        completion_tokens: number("completion_tokens")?,
        total_tokens: number("total_tokens")?,
    })
}

fn stop_reason(value: &str) -> StopReason {
    match value {
        "stop" => StopReason::Stop,
        "length" => StopReason::Length,
        "tool_calls" => StopReason::ToolCalls,
        "content_filter" => StopReason::ContentFilter,
        _ => StopReason::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_openai_frame;
    use crate::{ProviderEvent, SseFrame, StopReason, TokenUsage};

    #[test]
    fn parses_text_finish_and_usage() {
        let text = parse_openai_frame(&SseFrame {
            event: None,
            id: Some("chat-1".into()),
            data: r#"{"choices":[{"delta":{"content":"你好"},"finish_reason":null}]}"#.into(),
        })
        .expect("文本 delta 应能解析");
        assert_eq!(
            text,
            vec![ProviderEvent::TextDelta {
                text: "你好".into()
            }]
        );

        let finish = parse_openai_frame(&SseFrame {
            event: None,
            id: None,
            data: r#"{"choices":[{"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}"#
                .into(),
        })
        .expect("finish 应能解析");
        assert_eq!(
            finish,
            vec![
                ProviderEvent::Finished {
                    reason: StopReason::Stop
                },
                ProviderEvent::Usage {
                    usage: TokenUsage {
                        prompt_tokens: 10,
                        completion_tokens: 2,
                        total_tokens: 12
                    }
                }
            ]
        );
    }

    #[test]
    fn parses_tool_call_and_unknown_finish_reason() {
        let events = parse_openai_frame(&SseFrame {
            event: None,
            id: None,
            data: serde_json::json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call-1",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":"
                            }
                        }]
                    },
                    "finish_reason": "future_reason"
                }]
            })
            .to_string(),
        })
        .expect("tool call 应能解析");
        assert!(matches!(events[0], ProviderEvent::ToolCallDelta { .. }));
        assert_eq!(
            events[1],
            ProviderEvent::Finished {
                reason: StopReason::Unknown
            }
        );
    }

    #[test]
    fn malformed_json_is_protocol_error() {
        let error = parse_openai_frame(&SseFrame {
            event: None,
            id: None,
            data: "not-json".into(),
        })
        .expect_err("非法 JSON 必须失败");
        assert!(matches!(error, crate::ProviderError::Protocol(_)));
    }

    #[test]
    fn ignores_null_usage_in_streaming_chunk() {
        let events = parse_openai_frame(&SseFrame {
            event: None,
            id: None,
            data: r#"{"choices":[{"delta":{"content":"继续"}}],"usage":null}"#.into(),
        })
        .expect("usage=null 不是协议错误");
        assert_eq!(
            events,
            vec![ProviderEvent::TextDelta {
                text: "继续".into()
            }]
        );
    }
}
