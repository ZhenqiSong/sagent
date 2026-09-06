//! Provider 请求、流式事件和完成结果。

use sagent_types::{SessionId, TurnId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

/// assistant 消息中需要原样重放给 Provider 的函数调用。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderToolCall {
    /// Provider 分配的调用 ID。
    pub id: String,
    /// Provider 识别的函数名。
    pub name: String,
    /// 已解析的 JSON 参数。
    pub arguments: Value,
}

/// 发送给 Provider 的一条消息。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderMessage {
    /// 消息角色。
    pub role: ProviderRole,
    /// 消息文本。
    pub content: String,
    /// Tool 消息关联的调用 ID；普通消息为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// assistant 消息发起的完整函数调用；tool 消息保持为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ProviderToolCall>,
}

/// 一次模型请求的 provider-neutral 表示。
///
/// `request_id` 使用字符串是为了让 Provider 不依赖 Agent 的命令模块；Runtime 负责将
/// 自身的 RequestId 转换成稳定文本。API key 刻意不属于该结构，避免被序列化或日志记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderRequest {
    /// 会话关联信息，仅用于诊断和事件关联。
    pub session_id: SessionId,
    /// 当前回合。
    pub turn_id: TurnId,
    /// Runtime 生成的请求标识文本。
    pub request_id: String,
    /// Provider 模型名称。
    pub model: String,
    /// 按模型上下文顺序排列的消息。
    pub messages: Vec<ProviderMessage>,
    /// 发送给模型的 OpenAI-compatible function schemas。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    /// 允许模型使用的工具 schema。
    pub tools: Vec<Value>,
    /// 可选采样温度。
    pub temperature: Option<f32>,
    /// 是否请求流式响应。
    pub stream: bool,
}

/// Provider 发出的中性流式事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderEvent {
    /// 模型文本增量；Runtime 将其转换为瞬态 ModelTextDelta。
    TextDelta { text: String },
    /// 流式工具调用片段；Runtime 会先完整聚合并校验，再交由工具/审批回环执行。
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
    /// Provider 宣布的停止原因。
    pub reason: StopReason,
    /// Provider 返回的 token 统计。
    pub usage: Option<TokenUsage>,
    /// 上游 request id 只用于诊断和关联，不代表本地 RequestId。
    pub provider_request_id: Option<String>,
}

/// 模型调用的 token 使用统计。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// 输入 token 数量。
    pub prompt_tokens: u64,
    /// 输出 token 数量。
    pub completion_tokens: u64,
    /// 输入与输出总和。
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
                tool_calls: vec![],
            }],
            tools: vec![],
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
