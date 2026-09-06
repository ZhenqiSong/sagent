use async_trait::async_trait;
use sagent_provider::mock::{MockSseChunk, MockSseServer};
use sagent_provider::{
    ModelProvider, OpenAiCompatibleProvider, ProviderError, ProviderEvent, ProviderEventSink,
    ProviderMessage, ProviderRequest, ProviderRole, StopReason,
};
use sagent_types::{SessionId, TurnId};
use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Sink {
    events: Vec<ProviderEvent>,
}

#[async_trait]
impl ProviderEventSink for Sink {
    async fn emit(&mut self, event: ProviderEvent) -> Result<(), ProviderError> {
        self.events.push(event);
        Ok(())
    }
}

fn request(endpoint: String) -> ProviderRequest {
    let _ = endpoint;
    ProviderRequest {
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
    }
}

#[derive(Clone, Debug)]
struct CapturedRequest {
    headers: String,
    body: Vec<u8>,
}

struct TestServer {
    url: String,
    captured: Arc<Mutex<Option<CapturedRequest>>>,
    task: JoinHandle<Result<(), String>>,
}

impl TestServer {
    async fn spawn(status: &str, content_type: &str, body: Vec<u8>) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let captured = Arc::new(Mutex::new(None));
        let captured_for_task = Arc::clone(&captured);
        let status = status.to_owned();
        let content_type = content_type.to_owned();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener
                .accept()
                .await
                .map_err(|error| format!("accept failed: {error}"))?;
            let (headers, mut remainder) = read_headers(&mut stream)
                .await
                .map_err(|error| format!("read request failed: {error}"))?;
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then_some(value)
                })
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            while remainder.len() < content_length {
                let mut buffer = [0_u8; 1024];
                let count = stream
                    .read(&mut buffer)
                    .await
                    .map_err(|error| format!("read request body failed: {error}"))?;
                if count == 0 {
                    break;
                }
                remainder.extend_from_slice(&buffer[..count]);
            }
            *captured_for_task.lock().expect("capture lock") = Some(CapturedRequest {
                headers,
                body: remainder[..remainder.len().min(content_length)].to_vec(),
            });
            let response_headers = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(response_headers.as_bytes())
                .await
                .map_err(|error| format!("write headers failed: {error}"))?;
            stream
                .write_all(&body)
                .await
                .map_err(|error| format!("write body failed: {error}"))?;
            Ok(())
        });
        Ok(Self {
            url: format!("http://{address}"),
            captured,
            task,
        })
    }

    async fn wait(self) -> Result<(), String> {
        self.task
            .await
            .map_err(|error| format!("server task failed: {error}"))?
    }

    fn captured(&self) -> CapturedRequest {
        self.captured
            .lock()
            .expect("capture lock")
            .clone()
            .expect("request should be captured")
    }
}

async fn read_headers(stream: &mut TcpStream) -> io::Result<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let end;
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request ended",
            ));
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            end = position + 4;
            break;
        }
        if bytes.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "headers too large",
            ));
        }
    }
    let headers = String::from_utf8(bytes[..end].to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "headers not UTF-8"))?;
    Ok((headers, bytes[end..].to_vec()))
}

#[tokio::test(flavor = "current_thread")]
async fn streams_openai_sse_and_returns_finish() {
    let server = MockSseServer::spawn(vec![MockSseChunk::text(include_str!(
        "fixtures/provider/normal_text.sse"
    ))])
    .await
    .expect("mock server");
    let provider = OpenAiCompatibleProvider::from_endpoint(&server.url(), "secret-key")
        .expect("provider config");
    let mut sink = Sink::default();

    let finish = provider
        .stream(request(server.url()), &mut sink, CancellationToken::new())
        .await
        .expect("SSE should complete");
    server.wait().await.expect("server should complete");

    assert_eq!(finish.reason, StopReason::Stop);
    assert!(sink.events.iter().any(|event| matches!(
        event,
        ProviderEvent::TextDelta { text } if text == "你好"
    )));
}

#[tokio::test(flavor = "current_thread")]
async fn sends_bearer_header_and_openai_request_body() {
    let server = TestServer::spawn(
        "200 OK",
        "text/event-stream",
        include_bytes!("fixtures/provider/normal_text.sse").to_vec(),
    )
    .await
    .expect("test server");
    let provider = OpenAiCompatibleProvider::from_endpoint(&server.url, "secret-key")
        .expect("provider config");
    let mut sink = Sink::default();
    provider
        .stream(
            request(server.url.clone()),
            &mut sink,
            CancellationToken::new(),
        )
        .await
        .expect("request should complete");
    let captured = server.captured();
    server.wait().await.expect("server should complete");
    let lower_headers = captured.headers.to_ascii_lowercase();
    assert!(lower_headers.contains("authorization: bearer secret-key"));
    assert!(lower_headers.contains("accept: text/event-stream"));
    let json: serde_json::Value = serde_json::from_slice(&captured.body).expect("JSON body");
    assert_eq!(json["model"], "test-model");
    assert_eq!(json["stream"], true);
    assert_eq!(json["stream_options"]["include_usage"], true);
    assert!(!String::from_utf8_lossy(&captured.body).contains("secret-key"));
}

#[tokio::test(flavor = "current_thread")]
async fn maps_rate_limit_and_non_sse_response() {
    let server = TestServer::spawn(
        "429 Too Many Requests",
        "application/json",
        br#"{}"#.to_vec(),
    )
    .await
    .expect("test server");
    let provider = OpenAiCompatibleProvider::from_endpoint(&server.url, "key").unwrap();
    let mut sink = Sink::default();
    let error = provider
        .stream(
            request(server.url.clone()),
            &mut sink,
            CancellationToken::new(),
        )
        .await
        .expect_err("429 should fail");
    server.wait().await.expect("server should complete");
    assert!(matches!(error, ProviderError::RateLimited { .. }));

    let server = TestServer::spawn("200 OK", "application/json", br#"{}"#.to_vec())
        .await
        .expect("test server");
    let provider = OpenAiCompatibleProvider::from_endpoint(&server.url, "key").unwrap();
    let error = provider
        .stream(
            request(server.url.clone()),
            &mut sink,
            CancellationToken::new(),
        )
        .await
        .expect_err("non-SSE should fail");
    server.wait().await.expect("server should complete");
    assert!(matches!(error, ProviderError::Protocol(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_stops_a_slow_response() {
    let server = MockSseServer::spawn(vec![MockSseChunk::delayed(
        include_str!("fixtures/provider/normal_text.sse"),
        Duration::from_secs(5),
    )])
    .await
    .expect("mock server");
    let endpoint = server.url();
    drop(server);
    let provider = Arc::new(
        OpenAiCompatibleProvider::from_endpoint(&endpoint, "key").expect("provider config"),
    );
    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let provider_for_task = Arc::clone(&provider);
    let task = tokio::spawn(async move {
        let mut sink = Sink::default();
        provider_for_task
            .stream(request(String::new()), &mut sink, cancel_for_task)
            .await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    cancel.cancel();
    assert!(matches!(
        task.await.expect("provider task"),
        Err(ProviderError::Cancelled)
    ));
}
