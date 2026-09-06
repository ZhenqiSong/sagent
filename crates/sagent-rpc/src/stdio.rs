//! 标准输入输出上的异步 NDJSON JSON-RPC transport。

use std::io;

use sagent_protocol::{
    DispatchService, EventParams, JsonRpcError, JsonRpcEvent, JsonRpcRequest, JsonRpcResponse,
    ProtocolError, ProtocolFeatures, RequestId,
};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::connection::ConnectionState;

/// 单行请求最大字节数，防止 transport 无界读取。
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
const REQUEST_CHANNEL_CAPACITY: usize = 64;
const OUTBOUND_CHANNEL_CAPACITY: usize = 256;

/// 输出帧的优先级；瞬态 delta 将由后续 event-forwarder 使用合并/丢弃策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FrameClass {
    /// response、审批和 Turn 终态不能丢失。
    Critical,
    /// delta、usage 等允许在拥塞时降级处理。
    #[allow(dead_code)] // 4.6 后续 event-forwarder 接入 transient delta 时启用。
    Transient,
}

/// 已经序列化完成的一条 stdout NDJSON 帧。
struct OutboundFrame {
    bytes: Vec<u8>,
    class: FrameClass,
}

impl OutboundFrame {
    /// 序列化完整 JSON 后追加换行，确保 writer 每次处理一条协议记录。
    fn from_value<T: Serialize>(value: &T, class: FrameClass) -> io::Result<Self> {
        let mut bytes = serde_json::to_vec(value)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        Ok(Self { bytes, class })
    }

    /// 创建不可丢弃的控制帧。
    fn critical<T: Serialize>(value: &T) -> io::Result<Self> {
        Self::from_value(value, FrameClass::Critical)
    }
}

/// reader 送给 dispatcher 的输入；解析失败也排队，保持响应顺序。
enum InboundFrame {
    Request(JsonRpcRequest),
    Response(JsonRpcResponse<Value>),
}

/// 读到换行前只保留上限以内的字节；超限后继续消费到换行，避免污染下一帧。
enum ReadFrame {
    Data(Vec<u8>),
    Oversized,
}

/// 读取一条受限 NDJSON 帧。
///
/// 不使用无界的 `read_line`：当客户端不发送换行时，函数最多保留 1 MiB，之后
/// 只丢弃输入直到换行或 EOF，从而把内存使用限制在固定范围内。
async fn read_frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<ReadFrame>> {
    let mut frame = Vec::new();
    let mut oversized = false;

    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return if frame.is_empty() && !oversized {
                Ok(None)
            } else if oversized {
                Ok(Some(ReadFrame::Oversized))
            } else {
                Ok(Some(ReadFrame::Data(frame)))
            };
        }

        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(buffer.len(), |position| position + 1);

        if !oversized {
            let remaining = MAX_FRAME_BYTES.saturating_sub(frame.len());
            if consumed > remaining {
                oversized = true;
            } else {
                frame.extend_from_slice(&buffer[..consumed]);
            }
        }
        reader.consume(consumed);

        if newline.is_some() {
            return if oversized {
                Ok(Some(ReadFrame::Oversized))
            } else {
                Ok(Some(ReadFrame::Data(frame)))
            };
        }
    }
}

/// reader task：只读 stdin 和解析请求，不直接操作 stdout。
async fn reader_loop<R: AsyncBufRead + Unpin>(
    mut reader: R,
    request_tx: mpsc::Sender<InboundFrame>,
    cancellation: CancellationToken,
) -> io::Result<()> {
    loop {
        let frame = tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            frame = read_frame(&mut reader) => frame?,
        };
        let Some(frame) = frame else {
            return Ok(());
        };

        let inbound = match frame {
            ReadFrame::Oversized => InboundFrame::Response(JsonRpcResponse::failure(
                RequestId::Null,
                ProtocolError::InvalidParams(format!(
                    "request frame exceeds {MAX_FRAME_BYTES} bytes"
                ))
                .to_jsonrpc(),
            )),
            ReadFrame::Data(bytes) => {
                let bytes = trim_line_ending(&bytes);
                match serde_json::from_slice::<JsonRpcRequest>(bytes) {
                    Ok(request) => InboundFrame::Request(request),
                    Err(_) => InboundFrame::Response(JsonRpcResponse::failure(
                        RequestId::Null,
                        JsonRpcError {
                            code: -32700,
                            message: "parse error".to_owned(),
                            data: None,
                        },
                    )),
                }
            }
        };

        // 有界 request channel 同时限制输入积压；慢 dispatcher 会反压 reader，
        // 但不会让内存随客户端发送速度无限增长。
        if request_tx.send(inbound).await.is_err() {
            return Ok(());
        }
    }
}

/// dispatcher task：独占 ConnectionState，把 response 排队给唯一 writer。
async fn dispatcher_loop<S: DispatchService + Send + 'static>(
    mut request_rx: mpsc::Receiver<InboundFrame>,
    outbound_tx: mpsc::Sender<OutboundFrame>,
    service: S,
    mut connection: ConnectionState,
    cancellation: CancellationToken,
) -> io::Result<()> {
    while let Some(inbound) = tokio::select! {
        _ = cancellation.cancelled() => None,
        inbound = request_rx.recv() => inbound,
    } {
        let response = match inbound {
            InboundFrame::Request(request) => connection.dispatch(request, &service),
            InboundFrame::Response(response) => Some(response),
        };

        if let Some(response) = response {
            let frame = OutboundFrame::critical(&response)?;
            if outbound_tx.send(frame).await.is_err() {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// writer task：stdout 的唯一所有者，保证每帧完整写入并 flush。
async fn writer_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut outbound_rx: mpsc::Receiver<OutboundFrame>,
    cancellation: CancellationToken,
) -> io::Result<()> {
    while let Some(frame) = tokio::select! {
        _ = cancellation.cancelled() => None,
        frame = outbound_rx.recv() => frame,
    } {
        // 当前步骤还没有产生 transient event，但读取 class 可以确保后续 delta
        // 降级策略接入时仍由 writer 统一处理，不会出现第二个 stdout 写入者。
        match frame.class {
            FrameClass::Critical | FrameClass::Transient => {}
        }
        writer.write_all(&frame.bytes).await?;
        writer.flush().await?;
    }
    Ok(())
}

/// 异步运行一条 stdio 连接。
///
/// reader、dispatcher 和 writer 分别运行在独立 Tokio task 中；所有输出都必须先
/// 进入有界 outbound channel。reader EOF 后关闭请求流，dispatcher 排空已读请求，
/// 最后关闭 writer；任一任务失败都会取消同一 connection scope。
pub async fn run<R, W, S>(
    reader: R,
    writer: W,
    service: S,
    connection: ConnectionState,
) -> io::Result<()>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
    S: DispatchService + Send + 'static,
{
    let (request_tx, request_rx) = mpsc::channel(REQUEST_CHANNEL_CAPACITY);
    let (outbound_tx, outbound_rx) = mpsc::channel(OUTBOUND_CHANNEL_CAPACITY);
    outbound_tx
        .send(ready_frame()?)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "writer task stopped"))?;

    let cancellation = CancellationToken::new();
    let reader_task = tokio::spawn(reader_loop(reader, request_tx, cancellation.child_token()));
    let dispatcher_task = tokio::spawn(dispatcher_loop(
        request_rx,
        outbound_tx,
        service,
        connection,
        cancellation.child_token(),
    ));
    let writer_task = tokio::spawn(writer_loop(writer, outbound_rx, cancellation.child_token()));

    let mut reader_task = reader_task;
    let mut dispatcher_task = dispatcher_task;
    let mut writer_task = writer_task;

    // writer 失败时不能等待 stdin EOF；先取消并终止其它 task，避免 dispatcher
    // 永久阻塞在已经失效的 outbound channel 上。
    tokio::select! {
        result = &mut writer_task => {
            let writer_result = join_result(result);
            if writer_result.is_err() {
                cancellation.cancel();
                reader_task.abort();
                dispatcher_task.abort();
            }
            writer_result
        }
        result = &mut reader_task => {
            let reader_result = join_result(result);
            if reader_result.is_err() {
                cancellation.cancel();
                dispatcher_task.abort();
                writer_task.abort();
                return reader_result;
            }
            let dispatcher_result = join_result(dispatcher_task.await);
            if dispatcher_result.is_err() {
                cancellation.cancel();
                writer_task.abort();
                return dispatcher_result;
            }
            join_result(writer_task.await)
        }
        result = &mut dispatcher_task => {
            let dispatcher_result = join_result(result);
            if dispatcher_result.is_err() {
                cancellation.cancel();
                reader_task.abort();
                writer_task.abort();
                return dispatcher_result;
            }
            // request channel 只有在 reader 正常结束后才会关闭；此时 writer 仍需
            // 排空已经进入 outbound channel 的 response，不能在这里提前 abort。
            let reader_result = join_result(reader_task.await);
            if reader_result.is_err() {
                cancellation.cancel();
                writer_task.abort();
                return reader_result;
            }
            join_result(writer_task.await)
        }
    }
}

fn ready_frame() -> io::Result<OutboundFrame> {
    OutboundFrame::critical(&JsonRpcEvent {
        jsonrpc: "2.0".to_owned(),
        method: "event".to_owned(),
        params: EventParams {
            event_type: "gateway.ready".to_owned(),
            payload: ProtocolFeatures::available(),
        },
    })
}

fn join_result(result: Result<io::Result<()>, tokio::task::JoinError>) -> io::Result<()> {
    result.map_err(|error| io::Error::other(format!("stdio task failed: {error}")))?
}

fn trim_line_ending(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

#[cfg(test)]
mod tests {
    use std::{
        io::Cursor,
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll},
    };

    use sagent_protocol::{
        GatewayPingResult, SessionCreateParams, SessionCreateResult, SessionCreateService,
        SessionListParams, SessionListResult, SessionReadService, SessionResumeParams,
        SessionResumeResult,
    };
    use tokio::io::AsyncWrite;

    use super::{MAX_FRAME_BYTES, run};
    use crate::connection::ConnectionState;

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl AsyncWrite for SharedWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.0
                .lock()
                .expect("测试 writer 不应中毒")
                .extend_from_slice(bytes);
            Poll::Ready(Ok(bytes.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    struct FakeService;
    impl sagent_protocol::GatewayService for FakeService {
        fn ping(&self) -> GatewayPingResult {
            GatewayPingResult {
                ok: true,
                protocol_version: 1,
            }
        }
    }
    impl SessionReadService for FakeService {
        fn list_sessions(
            &self,
            _: &SessionListParams,
        ) -> Result<SessionListResult, sagent_protocol::ProtocolError> {
            Ok(SessionListResult {
                sessions: vec![],
                limit: 50,
                offset: 0,
            })
        }
        fn resume_session(
            &self,
            _: &SessionResumeParams,
        ) -> Result<SessionResumeResult, sagent_protocol::ProtocolError> {
            Err(sagent_protocol::ProtocolError::SessionNotFound(
                "not-used".to_owned(),
            ))
        }
    }

    impl SessionCreateService for FakeService {
        fn create_session(
            &self,
            _: &SessionCreateParams,
        ) -> Result<SessionCreateResult, sagent_protocol::ProtocolError> {
            Err(sagent_protocol::ProtocolError::Internal(
                "create is not used by this fake".to_owned(),
            ))
        }
    }

    async fn run_memory(input: Vec<u8>) -> Vec<serde_json::Value> {
        let output = Arc::new(Mutex::new(Vec::new()));
        let writer = SharedWriter(output.clone());
        run(
            tokio::io::BufReader::new(Cursor::new(input)),
            writer,
            FakeService,
            ConnectionState::new(),
        )
        .await
        .expect("stdio 应成功");

        String::from_utf8(output.lock().expect("测试 writer 不应中毒").clone())
            .expect("输出应为 UTF-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("每一行必须是 JSON"))
            .collect()
    }

    #[tokio::test]
    async fn emits_ready_then_ping_and_ignores_notification_response() {
        let frames = run_memory(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"gateway.ping\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"gateway.ping\"}\n".to_vec(),
        )
        .await;
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0]["params"]["type"], "gateway.ready");
        assert_eq!(frames[1]["id"], 1);
    }

    #[tokio::test]
    async fn malformed_json_returns_parse_error() {
        let frames = run_memory(b"not-json\n".to_vec()).await;
        assert_eq!(frames[1]["error"]["code"], -32700);
    }

    #[test]
    fn frame_limit_is_one_megabyte() {
        assert_eq!(MAX_FRAME_BYTES, 1024 * 1024);
    }

    #[tokio::test]
    async fn oversized_frame_returns_error_and_next_request_is_processed() {
        let input = format!(
            "{}\n{{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"gateway.ping\"}}\n",
            "x".repeat(MAX_FRAME_BYTES)
        );
        let frames = run_memory(input.into_bytes()).await;
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[1]["error"]["code"], -32602);
        assert_eq!(frames[2]["id"], 9);
        assert_eq!(frames[2]["result"]["ok"], true);
    }
}
