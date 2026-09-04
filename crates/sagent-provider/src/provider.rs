//! Provider trait 和事件接收器。

use crate::{ProviderError, ProviderEvent, ProviderFinish, ProviderRequest};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Runtime 接收 Provider 事件的窄接口。
///
/// Sink 不暴露 Store，也不允许 Provider 取得 Actor 内部状态。后续 runtime adapter 会
/// 在 `emit` 中把 ProviderEvent 转换成 Actor 的 WorkerEvent。
#[async_trait]
pub trait ProviderEventSink: Send {
    async fn emit(&mut self, event: ProviderEvent) -> Result<(), ProviderError>;
}

/// 模型 Provider 的统一异步接口。
///
/// Provider 只负责请求模型、解析响应和回传事件；Turn/Message 的持久化、终态竞争和
/// cancellation 收口仍由 `sagent-runtime::SessionActor` 负责。
#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn stream(
        &self,
        request: ProviderRequest,
        sink: &mut dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ProviderFinish, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::{ModelProvider, ProviderEventSink};
    use crate::{
        ProviderEvent, ProviderFinish, ProviderMessage, ProviderRequest, ProviderRole, StopReason,
    };
    use async_trait::async_trait;
    use sagent_types::{SessionId, TurnId};
    use tokio_util::sync::CancellationToken;

    struct CollectingSink {
        events: Vec<ProviderEvent>,
    }

    #[async_trait]
    impl ProviderEventSink for CollectingSink {
        async fn emit(&mut self, event: ProviderEvent) -> Result<(), crate::ProviderError> {
            self.events.push(event);
            Ok(())
        }
    }

    struct FakeProvider;

    #[async_trait]
    impl ModelProvider for FakeProvider {
        async fn stream(
            &self,
            _request: ProviderRequest,
            sink: &mut dyn ProviderEventSink,
            _cancel: CancellationToken,
        ) -> Result<ProviderFinish, crate::ProviderError> {
            sink.emit(ProviderEvent::TextDelta { text: "ok".into() })
                .await?;
            Ok(ProviderFinish {
                reason: StopReason::Stop,
                usage: None,
                provider_request_id: None,
            })
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn provider_trait_object_can_stream_events() {
        let provider: Box<dyn ModelProvider> = Box::new(FakeProvider);
        let request = ProviderRequest {
            session_id: SessionId::new("session-1"),
            turn_id: TurnId::new(),
            request_id: "request-1".into(),
            model: "test-model".into(),
            messages: vec![ProviderMessage {
                role: ProviderRole::User,
                content: "hello".into(),
                tool_call_id: None,
            }],
            temperature: None,
            stream: true,
        };
        let mut sink = CollectingSink { events: Vec::new() };

        let finish = provider
            .stream(request, &mut sink, CancellationToken::new())
            .await
            .expect("fake provider 应能完成");

        assert_eq!(finish.reason, StopReason::Stop);
        assert_eq!(sink.events.len(), 1);
        assert!(matches!(sink.events[0], ProviderEvent::TextDelta { .. }));
    }
}
