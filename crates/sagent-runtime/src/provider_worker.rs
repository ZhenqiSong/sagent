//! 将 Provider 的模型无关事件桥接到 SessionActor mailbox。

use std::sync::Arc;

use async_trait::async_trait;
use sagent_agent::{PromptRole, PromptSnapshot, RequestId};
use sagent_provider::{
    ModelProvider, ProviderError, ProviderEvent, ProviderEventSink, ProviderMessage,
    ProviderRequest, ProviderToolCall, StopReason,
};
use sagent_types::TurnId;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::input::{ActorInput, WorkerEvent};
use crate::tool_call::{ToolCall, ToolCallAccumulator, ToolCallError};

/// 启动一个 Provider worker；worker 不接触 Store，只向 Actor mailbox 发送事件。
pub(crate) fn spawn_provider_worker(
    provider: Arc<dyn ModelProvider>,
    snapshot: PromptSnapshot,
    model: String,
    request_id: RequestId,
    tools: Vec<Value>,
    command_tx: mpsc::Sender<ActorInput>,
    cancellation: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let turn_id = snapshot.turn_id;
        // PromptSnapshot 在 Actor 中构造完成后才交给 Provider。worker 只做格式转换，
        // 这样流式回调无论何时到达，都不会看到被并发改写的会话上下文。
        let request = provider_request(&snapshot, &model, request_id, tools);
        let mut sink = RuntimeProviderSink::new(command_tx.clone(), turn_id);

        match provider.stream(request, &mut sink, cancellation).await {
            Ok(finish) => match sink.into_completion(finish.reason) {
                Ok((text, Some(calls))) => {
                    let _ = send_worker_event(
                        &command_tx,
                        WorkerEvent::ToolCalls {
                            turn_id,
                            text,
                            calls,
                        },
                    )
                    .await;
                }
                Ok((text, None)) => {
                    let _ =
                        send_worker_event(&command_tx, WorkerEvent::FinalText { turn_id, text })
                            .await;
                }
                Err(error) => {
                    let _ = send_worker_event(
                        &command_tx,
                        WorkerEvent::Failed {
                            turn_id,
                            reason: error,
                        },
                    )
                    .await;
                }
            },
            Err(ProviderError::Cancelled) => {
                let _ = send_worker_event(&command_tx, WorkerEvent::Cancelled { turn_id }).await;
            }
            Err(error) => {
                let _ = send_worker_event(
                    &command_tx,
                    WorkerEvent::Failed {
                        turn_id,
                        reason: error.to_string(),
                    },
                )
                .await;
            }
        }
    })
}

fn provider_request(
    snapshot: &PromptSnapshot,
    model: &str,
    request_id: RequestId,
    tools: Vec<Value>,
) -> ProviderRequest {
    // 这里刻意逐字段复制而不是把 PromptMessage 暴露给 provider crate：
    // agent 层定义“模型应该看到什么”，provider 层只负责“怎样发给某家 API”。
    ProviderRequest {
        session_id: snapshot.session_id.clone(),
        turn_id: snapshot.turn_id,
        request_id: request_id.to_string(),
        model: model.to_owned(),
        messages: snapshot
            .messages
            .iter()
            .map(|message| ProviderMessage {
                role: match message.role {
                    PromptRole::System => sagent_provider::ProviderRole::System,
                    PromptRole::User => sagent_provider::ProviderRole::User,
                    PromptRole::Assistant => sagent_provider::ProviderRole::Assistant,
                    PromptRole::Tool => sagent_provider::ProviderRole::Tool,
                },
                content: message.content.clone(),
                tool_call_id: message.tool_call_id.clone(),
                tool_calls: message
                    .tool_calls
                    .iter()
                    .map(|call| ProviderToolCall {
                        id: call.call_id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    })
                    .collect(),
            })
            .collect(),
        tools,
        temperature: None,
        stream: true,
    }
}

async fn send_worker_event(
    command_tx: &mpsc::Sender<ActorInput>,
    event: WorkerEvent,
) -> Result<(), ProviderError> {
    // mailbox 关闭意味着 Actor 已进入终态；把它映射成统一错误，使 Provider 的
    // 流循环能停止，而不是在已无人消费的通道上继续工作。
    command_tx
        .send(ActorInput::Worker(event))
        .await
        .map_err(|_| ProviderError::EventSinkClosed)
}

struct RuntimeProviderSink {
    command_tx: mpsc::Sender<ActorInput>,
    turn_id: TurnId,
    text: String,
    tool_calls: ToolCallAccumulator,
}

impl RuntimeProviderSink {
    fn new(command_tx: mpsc::Sender<ActorInput>, turn_id: TurnId) -> Self {
        Self {
            command_tx,
            turn_id,
            text: String::new(),
            tool_calls: ToolCallAccumulator::new(),
        }
    }

    fn into_completion(
        self,
        reason: StopReason,
    ) -> Result<(String, Option<Vec<ToolCall>>), String> {
        let Self {
            text, tool_calls, ..
        } = self;
        if tool_calls.is_empty() {
            if reason == StopReason::ToolCalls {
                return Err("Provider 声明了 tool_calls，但没有完整工具调用".into());
            }
            return Ok((text, None));
        }
        // ToolCallAccumulator 在流结束时才验证 JSON：单个 delta 往往只是半段
        // 参数，过早解析会把合法的分片协议误判为错误。
        let calls = tool_calls
            .finish()
            .map_err(|error: ToolCallError| error.to_string())?;
        Ok((text, Some(calls)))
    }
}

#[async_trait]
impl ProviderEventSink for RuntimeProviderSink {
    async fn emit(&mut self, event: ProviderEvent) -> Result<(), ProviderError> {
        match event {
            ProviderEvent::TextDelta { text } => {
                self.text.push_str(&text);
                send_worker_event(
                    &self.command_tx,
                    WorkerEvent::TextDelta {
                        turn_id: self.turn_id,
                        text,
                    },
                )
                .await
            }
            ProviderEvent::Usage { usage } => {
                send_worker_event(
                    &self.command_tx,
                    WorkerEvent::Usage {
                        turn_id: self.turn_id,
                        usage,
                    },
                )
                .await
            }
            ProviderEvent::Finished { .. } => Ok(()),
            ProviderEvent::ToolCallDelta {
                call_id,
                name,
                arguments_delta,
            } => self
                .tool_calls
                .push(call_id, name, arguments_delta)
                .map_err(|error| ProviderError::Protocol(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeProviderSink, provider_request};
    use sagent_agent::{PromptMessage, PromptRole, PromptSnapshot, RequestId, SystemPromptParts};
    use sagent_provider::{ProviderEvent, ProviderEventSink, StopReason};
    use sagent_types::{SessionId, TurnId};
    use tokio::sync::mpsc;

    #[test]
    fn maps_prompt_snapshot_to_provider_request() {
        let session_id = SessionId::new("session-1");
        let turn_id = TurnId::new();
        let system = SystemPromptParts {
            identity: "identity".into(),
            ..SystemPromptParts::default()
        };
        let snapshot = PromptSnapshot::new(
            session_id.clone(),
            turn_id,
            &system,
            vec![
                PromptMessage::new(PromptRole::System, "identity"),
                PromptMessage::new(PromptRole::User, "hello"),
            ],
        )
        .expect("snapshot 应有效");

        let request = provider_request(&snapshot, "test-model", RequestId::new(), vec![]);
        assert_eq!(request.session_id, session_id);
        assert_eq!(request.turn_id, turn_id);
        assert_eq!(request.model, "test-model");
        assert_eq!(request.messages.len(), 2);
        assert!(request.stream);
    }

    #[tokio::test]
    async fn aggregates_tool_call_deltas_until_provider_finish() {
        let (sender, _receiver) = mpsc::channel(8);
        let mut sink = RuntimeProviderSink::new(sender, TurnId::new());
        sink.emit(ProviderEvent::ToolCallDelta {
            call_id: "call-1".into(),
            name: Some("terminal".into()),
            arguments_delta: "{\"command\":".into(),
        })
        .await
        .unwrap();
        sink.emit(ProviderEvent::ToolCallDelta {
            call_id: "call-1".into(),
            name: None,
            arguments_delta: "\"pwd\"}".into(),
        })
        .await
        .unwrap();
        let (text, calls) = sink
            .into_completion(StopReason::ToolCalls)
            .expect("工具调用应能完成");
        let calls = calls.expect("应返回工具调用");
        assert!(text.is_empty());
        assert_eq!(calls[0].name, "terminal");
        assert_eq!(calls[0].arguments["command"], "pwd");
    }

    #[tokio::test]
    async fn invalid_tool_json_becomes_a_protocol_error() {
        let (sender, _receiver) = mpsc::channel(8);
        let mut sink = RuntimeProviderSink::new(sender, TurnId::new());
        sink.emit(ProviderEvent::ToolCallDelta {
            call_id: "call-1".into(),
            name: Some("terminal".into()),
            arguments_delta: "not-json".into(),
        })
        .await
        .unwrap();
        assert!(sink.into_completion(StopReason::ToolCalls).is_err());
    }
}
