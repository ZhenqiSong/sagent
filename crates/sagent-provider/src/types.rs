//! Provider 请求、流式事件和完成结果。

use sagent_types::{SessionId, TurnId};
use serde::{Deserialize, Serialize};

/// Provider 可理解的消息角色。
///
/// 这是 Provider 边界自己的 DTO，不直接暴露 `sagent-agent::PromptRole`，使 HTTP/SSE
/// adapter 不依赖 Agent 状态机的内部表示。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderRole {
    System,
    User,
    Assistant,
    Tool,
}

/// 发送给 Provider 的一条消息。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderMessage {
    pub role: ProviderRole,
    pub content: String,
    /// Tool 消息关联的调用 ID；普通消息为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// 一次模型请求的 provider-neutral 表示。
///
/// `request_id` 使用字符串是为了让 Provider 不依赖 Agent 的命令模块；Runtime 负责将
/// 自身的 RequestId 转换成稳定文本。API key 刻意不属于该结构，避免被序列化或日志记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub request_id: String,
    pub model: String,
    pub messages: Vec<ProviderMessage>,
    pub temperature: Option<f32>,
    pub stream: bool,
}

/// Provider 发出的中性流式事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderEvent {
    /// 模型文本增量；Runtime 将其转换为瞬态 ModelTextDelta。
    TextDelta { text: String },
    /// 工具调用增量只在 4.4 保留中性结构，实际执行属于 4.5。
    ToolCallDelta {
        call_id: String,
        name: Option<String>,
        arguments_delta: String,
    },
    /// 结构化 token 统计，不拼接到 assistant 消息正文。
    Usage { usage: TokenUsage },
    /// Provider 已收到明确的完成信号。
    Finished { reason: StopReason },
}

/// Provider 完成原因。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Unknown,
}

/// Provider 调用结束后的结构化结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderFinish {
    pub reason: StopReason,
    pub usage: Option<TokenUsage>,
    /// 上游 request id 只用于诊断和关联，不代表本地 RequestId。
    pub provider_request_id: Option<String>,
}

/// 模型调用的 token 使用统计。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::{
        ProviderEvent, ProviderMessage, ProviderRequest, ProviderRole, StopReason, TokenUsage,
    };
    use sagent_types::{SessionId, TurnId};

    #[test]
    fn provider_event_round_trips_with_stable_tag() {
        let event = ProviderEvent::TextDelta {
            text: "你好".into(),
        };

        let value = serde_json::to_value(&event).expect("事件应能序列化");
        assert_eq!(value["type"], "text_delta");

        let decoded: ProviderEvent = serde_json::from_value(value).expect("事件应能反序列化");
        assert_eq!(decoded, event);
    }

    #[test]
    fn provider_request_round_trips_without_secret_field() {
        let request = ProviderRequest {
            session_id: SessionId::new("session-1"),
            turn_id: TurnId::new(),
            request_id: "request-1".into(),
            model: "test-model".into(),
            messages: vec![ProviderMessage {
                role: ProviderRole::User,
                content: "你好".into(),
                tool_call_id: None,
            }],
            temperature: Some(0.2),
            stream: true,
        };

        let json = serde_json::to_string(&request).expect("请求应能序列化");
        assert!(!json.contains("api_key"));
        assert!(!json.contains("Authorization"));

        let decoded: ProviderRequest = serde_json::from_str(&json).expect("请求应能反序列化");
        assert_eq!(decoded, request);
    }

    #[test]
    fn usage_and_stop_reason_are_preserved() {
        let event = ProviderEvent::Usage {
            usage: TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        };
        let finish = StopReason::ContentFilter;

        let decoded_event: ProviderEvent =
            serde_json::from_str(&serde_json::to_string(&event).unwrap()).unwrap();
        let decoded_reason: StopReason =
            serde_json::from_str(&serde_json::to_string(&finish).unwrap()).unwrap();

        assert_eq!(decoded_event, event);
        assert_eq!(decoded_reason, finish);
    }
}
