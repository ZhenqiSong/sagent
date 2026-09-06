//! 测试用 Provider 和最小本地 SSE Server。
//!
//! 这些类型只用于确定性测试：MockProvider 按脚本发出 provider-neutral 事件，
//! MockSseServer 用原始 TCP 返回固定 HTTP/SSE 字节，不访问真实网络服务。

use crate::{
    ModelProvider, ProviderError, ProviderEvent, ProviderEventSink, ProviderFinish,
    ProviderRequest, StopReason, TokenUsage,
};
use async_trait::async_trait;
use std::{io, net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::sleep,
};
use tokio_util::sync::CancellationToken;

/// MockProvider 的动作脚本。
#[derive(Debug, Clone, PartialEq)]
pub enum MockAction {
    /// 发送一段文本增量。
    Delta(String),
    /// 发送一段中性的工具调用增量；Runtime 会在流结束后聚合并执行它。
    ToolCallDelta {
        call_id: String,
        name: Option<String>,
        arguments_delta: String,
    },
    /// 发送 token usage。
    Usage(TokenUsage),
    /// 等待指定时间，同时响应取消。
    Delay(Duration),
    /// 等待取消，不会产生完成事件。
    WaitForCancel,
    /// 发送明确完成事件并结束 Provider 调用。
    Finish(StopReason),
    /// 立即返回指定错误。
    Fail(ProviderError),
}

/// 按固定动作产生 ProviderEvent 的测试 Provider。
#[derive(Debug, Clone, Default)]
pub struct MockProvider {
    actions: Vec<MockAction>,
}

impl MockProvider {
    /// 创建一个动作脚本。
    pub fn new(actions: impl Into<Vec<MockAction>>) -> Self {
        Self {
            actions: actions.into(),
        }
    }

    /// 返回当前脚本的只读视图，便于测试诊断。
    pub fn actions(&self) -> &[MockAction] {
        &self.actions
    }
}

#[async_trait]
impl ModelProvider for MockProvider {
    async fn stream(
        &self,
        request: ProviderRequest,
        sink: &mut dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ProviderFinish, ProviderError> {
        let mut usage = None;

        for action in &self.actions {
            if cancel.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }

            match action {
                MockAction::Delta(text) => {
                    sink.emit(ProviderEvent::TextDelta { text: text.clone() })
                        .await?;
                }
                MockAction::ToolCallDelta {
                    call_id,
                    name,
                    arguments_delta,
                } => {
                    sink.emit(ProviderEvent::ToolCallDelta {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments_delta: arguments_delta.clone(),
                    })
                    .await?;
                }
                MockAction::Usage(value) => {
                    usage = Some(*value);
                    sink.emit(ProviderEvent::Usage { usage: *value }).await?;
                }
                MockAction::Delay(duration) => {
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                        _ = sleep(*duration) => {}
                    }
                }
                MockAction::WaitForCancel => {
                    cancel.cancelled().await;
                    return Err(ProviderError::Cancelled);
                }
                MockAction::Finish(reason) => {
                    sink.emit(ProviderEvent::Finished { reason: *reason })
                        .await?;
                    return Ok(ProviderFinish {
                        reason: *reason,
                        usage,
                        provider_request_id: Some(format!("mock-{}", request.request_id)),
                    });
                }
                MockAction::Fail(error) => return Err(error.clone()),
            }
        }

        Err(ProviderError::IncompleteStream)
    }
}

/// Mock SSE Server 写入的一个字节块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockSseChunk {
    pub bytes: Vec<u8>,
    pub delay_before: Option<Duration>,
}

impl MockSseChunk {
    /// 创建立即写入的 UTF-8 chunk。
    pub fn text(value: impl Into<String>) -> Self {
        Self {
            bytes: value.into().into_bytes(),
            delay_before: None,
        }
    }

    /// 创建带发送前延迟的 UTF-8 chunk。
    pub fn delayed(value: impl Into<String>, delay: Duration) -> Self {
        Self {
            bytes: value.into().into_bytes(),
            delay_before: Some(delay),
        }
    }

    /// 创建原始字节 chunk，用于模拟任意分片。
    pub fn bytes(value: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: value.into(),
            delay_before: None,
        }
    }
}

/// 只服务一个连接的本地 HTTP/SSE 测试服务器。
pub struct MockSseServer {
    address: SocketAddr,
    task: JoinHandle<Result<(), String>>,
}

impl MockSseServer {
    /// 启动 200 text/event-stream 响应。
    pub async fn spawn(chunks: Vec<MockSseChunk>) -> io::Result<Self> {
        Self::spawn_response(200, "OK", "text/event-stream", chunks).await
    }

    /// 启动指定状态码的 JSON 错误响应。
    pub async fn spawn_status(
        status: u16,
        reason: &str,
        body: impl Into<Vec<u8>>,
    ) -> io::Result<Self> {
        Self::spawn_response(
            status,
            reason,
            "application/json",
            vec![MockSseChunk::bytes(body)],
        )
        .await
    }

    async fn spawn_response(
        status: u16,
        reason: &str,
        content_type: &str,
        chunks: Vec<MockSseChunk>,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let reason = reason.to_owned();
        let content_type = content_type.to_owned();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener
                .accept()
                .await
                .map_err(|error| format!("accept failed: {error}"))?;
            read_request(&mut stream)
                .await
                .map_err(|error| format!("read request failed: {error}"))?;

            let content_length: usize = chunks.iter().map(|chunk| chunk.bytes.len()).sum();
            let headers = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n"
            );
            stream
                .write_all(headers.as_bytes())
                .await
                .map_err(|error| format!("write headers failed: {error}"))?;

            for chunk in chunks {
                if let Some(delay) = chunk.delay_before {
                    sleep(delay).await;
                }
                stream
                    .write_all(&chunk.bytes)
                    .await
                    .map_err(|error| format!("write body failed: {error}"))?;
                stream
                    .flush()
                    .await
                    .map_err(|error| format!("flush body failed: {error}"))?;
            }
            Ok(())
        });

        Ok(Self { address, task })
    }

    /// 返回可用于 HTTP client 的 base URL。
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// 等待服务器完成一次请求。
    pub async fn wait(self) -> Result<(), String> {
        self.task
            .await
            .map_err(|error| format!("mock server task failed: {error}"))?
    }
}

async fn read_request(stream: &mut TcpStream) -> io::Result<()> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 512];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..count]);
        if request.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP request headers too large",
            ));
        }
    }
    Ok(())
}
