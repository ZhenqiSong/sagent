use async_trait::async_trait;
use sagent_provider::{
    ModelProvider, ProviderError, ProviderEvent, ProviderEventSink, ProviderMessage,
    ProviderRequest, ProviderRole, StopReason, TokenUsage,
    mock::{MockAction, MockProvider, MockSseChunk, MockSseServer},
};
use sagent_types::{SessionId, TurnId};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

struct CollectingSink {
    events: Vec<ProviderEvent>,
}

#[async_trait]
impl ProviderEventSink for CollectingSink {
    async fn emit(&mut self, event: ProviderEvent) -> Result<(), ProviderError> {
        self.events.push(event);
        Ok(())
    }
}

fn request() -> ProviderRequest {
    ProviderRequest {
        session_id: SessionId::new("session-1"),
        turn_id: TurnId::new(),
        request_id: "request-1".into(),
        model: "mock-model".into(),
        messages: vec![ProviderMessage {
            role: ProviderRole::User,
            content: "你好".into(),
            tool_call_id: None,
            tool_calls: vec![],
        }],
        tools: vec![],
        temperature: None,
        stream: true,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn mock_provider_emits_normal_script_and_finish() {
    let provider = MockProvider::new([
        MockAction::Delta("你好".into()),
        MockAction::Delta("，Sagent".into()),
        MockAction::Usage(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 2,
            total_tokens: 12,
        }),
        MockAction::Finish(StopReason::Stop),
    ]);
    let mut sink = CollectingSink { events: Vec::new() };

    let finish = provider
        .stream(request(), &mut sink, CancellationToken::new())
        .await
        .expect("正常脚本应完成");

    assert_eq!(finish.reason, StopReason::Stop);
    assert_eq!(finish.usage.unwrap().total_tokens, 12);
    assert!(matches!(sink.events[0], ProviderEvent::TextDelta { .. }));
    assert!(matches!(sink.events[1], ProviderEvent::TextDelta { .. }));
    assert!(matches!(sink.events[2], ProviderEvent::Usage { .. }));
    assert!(matches!(sink.events[3], ProviderEvent::Finished { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn mock_provider_returns_incomplete_stream_without_finish() {
    let provider = MockProvider::new([MockAction::Delta("部分回答".into())]);
    let mut sink = CollectingSink { events: Vec::new() };

    let error = provider
        .stream(request(), &mut sink, CancellationToken::new())
        .await
        .expect_err("没有 finish 的脚本必须失败");

    assert_eq!(error, ProviderError::IncompleteStream);
    assert_eq!(sink.events.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn mock_provider_cancellation_stops_delayed_script() {
    let provider = MockProvider::new([
        MockAction::Delta("第一段".into()),
        MockAction::WaitForCancel,
        MockAction::Finish(StopReason::Stop),
    ]);
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    let task = tokio::spawn(async move {
        let mut sink = CollectingSink { events: Vec::new() };
        let result = provider.stream(request(), &mut sink, child).await;
        (result, sink.events)
    });

    tokio::task::yield_now().await;
    cancel.cancel();
    let (result, events) = timeout(Duration::from_secs(1), task)
        .await
        .expect("取消必须快速收口")
        .expect("Provider task 不应 panic");

    assert_eq!(result, Err(ProviderError::Cancelled));
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], ProviderEvent::TextDelta { .. }));
}

#[tokio::test(flavor = "current_thread")]
async fn mock_sse_server_writes_headers_and_fixture_body() {
    let fixture = include_str!("fixtures/provider/normal_text.sse");
    let server = MockSseServer::spawn(vec![MockSseChunk::text(fixture)])
        .await
        .expect("应能启动 Mock SSE Server");
    let mut stream = TcpStream::connect(server.url().strip_prefix("http://").unwrap())
        .await
        .expect("应能连接 Mock SSE Server");
    stream
        .write_all(b"GET /v1/chat/completions HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .expect("应能发送请求");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("应能读取响应");
    server.wait().await.expect("Mock SSE Server 应正常结束");

    let text = String::from_utf8(response).expect("响应应为 UTF-8");
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(text.contains("Content-Type: text/event-stream"));
    assert!(text.ends_with(fixture));
}

#[tokio::test(flavor = "current_thread")]
async fn mock_sse_server_preserves_network_chunk_boundaries() {
    let server = MockSseServer::spawn(vec![
        MockSseChunk::text("data: {\"choices\":[{\"delta\":{\"content\":\"你"),
        MockSseChunk::delayed("好\"}}]}\n\n", Duration::from_millis(1)),
        MockSseChunk::text("data: [DONE]\n\n"),
    ])
    .await
    .expect("应能启动 Mock SSE Server");
    let mut stream = TcpStream::connect(server.url().strip_prefix("http://").unwrap())
        .await
        .expect("应能连接 Mock SSE Server");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .expect("应能发送请求");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("应能读取响应");
    server.wait().await.expect("Mock SSE Server 应正常结束");

    let text = String::from_utf8(response).expect("响应应为 UTF-8");
    let you = text.find('你').expect("应包含第一个分片");
    let good = text.find('好').expect("应包含第二个分片");
    assert!(you < good, "分片合并后仍应保持顺序");
    assert!(text.contains("data: [DONE]"));
}

#[tokio::test(flavor = "current_thread")]
async fn mock_sse_server_can_return_rate_limit_and_server_error() {
    for (status, reason, fixture) in [
        (
            429,
            "Too Many Requests",
            include_str!("fixtures/provider/rate_limited.json"),
        ),
        (
            503,
            "Service Unavailable",
            include_str!("fixtures/provider/server_error.json"),
        ),
    ] {
        let server = MockSseServer::spawn_status(status, reason, fixture.as_bytes().to_vec())
            .await
            .expect("应能启动错误 Mock Server");
        let mut stream = TcpStream::connect(server.url().strip_prefix("http://").unwrap())
            .await
            .expect("应能连接错误 Mock Server");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("应能发送请求");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .expect("应能读取错误响应");
        server.wait().await.expect("错误 Mock Server 应正常结束");

        let text = String::from_utf8(response).expect("响应应为 UTF-8");
        assert!(text.starts_with(&format!("HTTP/1.1 {status} {reason}")));
        assert!(text.ends_with(fixture));
    }
}
