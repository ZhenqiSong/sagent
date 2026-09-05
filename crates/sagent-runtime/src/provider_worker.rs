//! 将 Provider 的模型无关事件桥接到 SessionActor mailbox。

use std::sync::Arc;

use async_trait::async_trait;
use sagent_agent::{PromptRole, PromptSnapshot, RequestId};
use sagent_provider::{
    ModelProvider, ProviderError, ProviderEvent, ProviderEventSink, ProviderMessage,
    ProviderRequest,
};
use sagent_types::TurnId;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::input::{ActorInput, WorkerEvent};

/// 启动一个 Provider worker；worker 不接触 Store，只向 Actor mailbox 发送事件。
pub(crate) fn spawn_provider_worker(
    provider: Arc<dyn ModelProvider>,
    snapshot: PromptSnapshot,
    model: String,
    request_id: RequestId,
    command_tx: mpsc::Sender<ActorInput>,
    cancellation: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let turn_id = snapshot.turn_id;
        let request = provider_request(&snapshot, &model, request_id);
        let mut sink = RuntimeProviderSink::new(command_tx.clone(), turn_id);

        match provider.stream(request, &mut sink, cancellation).await {
            Ok(_) => {
                let _ = send_worker_event(
                    &command_tx,
                    WorkerEvent::FinalText {
                        turn_id,
                        text: sink.into_text(),
                    },
                )
                .await;
            }
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
) -> ProviderRequest {
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
            })
            .collect(),
        temperature: None,
        stream: true,
    }
}

async fn send_worker_event(
    command_tx: &mpsc::Sender<ActorInput>,
    event: WorkerEvent,
) -> Result<(), ProviderError> {
    command_tx
        .send(ActorInput::Worker(event))
        .await
        .map_err(|_| ProviderError::EventSinkClosed)
}

struct RuntimeProviderSink {
    command_tx: mpsc::Sender<ActorInput>,
    turn_id: TurnId,
    text: String,
}

impl RuntimeProviderSink {
    fn new(command_tx: mpsc::Sender<ActorInput>, turn_id: TurnId) -> Self {
        Self {
            command_tx,
            turn_id,
            text: String::new(),
        }
    }

    fn into_text(self) -> String {
        self.text
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
            ProviderEvent::ToolCallDelta { .. } => Err(ProviderError::Protocol(
                "当前 runtime 尚未支持工具调用".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::provider_request;
    use sagent_agent::{PromptMessage, PromptRole, PromptSnapshot, RequestId, SystemPromptParts};
    use sagent_types::{SessionId, TurnId};

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

        let request = provider_request(&snapshot, "test-model", RequestId::new());
        assert_eq!(request.session_id, session_id);
        assert_eq!(request.turn_id, turn_id);
        assert_eq!(request.model, "test-model");
        assert_eq!(request.messages.len(), 2);
        assert!(request.stream);
    }
}
