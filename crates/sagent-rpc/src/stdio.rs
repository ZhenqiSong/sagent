//! 标准输入输出上的异步 NDJSON JSON-RPC transport。

use std::{any::Any, io};

use sagent_agent::{RequestId as RuntimeRequestId, UserInput};
use sagent_protocol::{
    DispatchService, EventParams, JsonRpcError, JsonRpcEvent, JsonRpcRequest, JsonRpcResponse,
    PromptSubmitParams, PromptSubmitResult, PromptSubmitStatus, ProtocolError, ProtocolFeatures,
    RequestId,
};
use sagent_runtime::RuntimeError;
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::connection::ConnectionState;
use crate::{event_bridge, service::RuntimePromptContext};

/// 单行请求最大字节数，防止 transport 无界读取。
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
const REQUEST_CHANNEL_CAPACITY: usize = 64;
const OUTBOUND_CHANNEL_CAPACITY: usize = 256;

/// 输出帧的优先级；瞬态 delta 将由后续 event-forwarder 使用合并/丢弃策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameClass {
    /// response、审批和 Turn 终态不能丢失。
    Critical,
    /// delta、usage 等允许在拥塞时降级处理。
    #[allow(dead_code)] // 4.6 后续 event-forwarder 接入 transient delta 时启用。
    Transient,
}

/// 已经序列化完成的一条 stdout NDJSON 帧。
pub(crate) struct OutboundFrame {
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
    pub(crate) fn critical<T: Serialize>(value: &T) -> io::Result<Self> {
        Self::from_value(value, FrameClass::Critical)
    }

    /// Runtime event 的 delta 是瞬态内容；终态和持久化确认仍作为 critical，避免
    /// 后续拥塞策略为了保护 stdout 而错误丢弃已提交的事实。
    pub(crate) fn event(event: &sagent_runtime::RuntimeEvent) -> io::Result<Self> {
        let class = match event.kind {
            sagent_runtime::RuntimeEventKind::ModelTextDelta { .. }
            | sagent_runtime::RuntimeEventKind::ModelUsage { .. }
            | sagent_runtime::RuntimeEventKind::SubscriberLagged { .. } => FrameClass::Transient,
            _ => FrameClass::Critical,
        };
        Self::from_value(&event_bridge::jsonrpc_event(event), class)
    }
}

/// 将 Runtime 内部错误收敛为协议可承诺的错误分类，绝不把 SQLite/Provider 细节上送。
fn runtime_error(error: RuntimeError) -> ProtocolError {
    match error {
        RuntimeError::Busy { session_id } => ProtocolError::SessionBusy {
            session_id: session_id.as_str().to_owned(),
        },
        RuntimeError::MailboxFull | RuntimeError::MailboxClosed | RuntimeError::ActorStopped => {
            ProtocolError::RuntimeUnavailable("session actor unavailable".to_owned())
        }
        RuntimeError::NoActiveTurn => ProtocolError::NoActiveTurn {
            session_id: "unknown".to_owned(),
            turn_id: None,
        },
        RuntimeError::Persistence(_) => {
            ProtocolError::StoreUnavailable("persistence failed".to_owned())
        }
        RuntimeError::InvalidLifecycle(_)
        | RuntimeError::RequiresTransition
        | RuntimeError::WorkerFailed(_)
        | RuntimeError::Approval(_) => {
            ProtocolError::Internal("runtime operation failed".to_owned())
        }
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
            InboundFrame::Request(request) if request.method == "prompt.submit" => {
                let runtime = (&service as &dyn Any)
                    .downcast_ref::<crate::service::RuntimeService>()
                    .map(crate::service::RuntimeService::prompt_context);
                match runtime {
                    Some(runtime) => {
                        dispatch_prompt_request(
                            request,
                            runtime,
                            &mut connection,
                            outbound_tx.clone(),
                            cancellation.child_token(),
                        )
                        .await
                    }
                    // 供 transport 单元测试使用的 fake 没有 Actor；仍交回 protocol，
                    // 从而保留握手 gate 与稳定的 runtime_unavailable 兼容响应。
                    None => connection.dispatch(request, &service),
                }
            }
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

/// 异步处理 `prompt.submit`，并用闸门维持 response → event 的协议顺序。
async fn dispatch_prompt_request(
    request: JsonRpcRequest,
    runtime: RuntimePromptContext,
    connection: &mut ConnectionState,
    outbound_tx: mpsc::Sender<OutboundFrame>,
    cancellation: CancellationToken,
) -> Option<JsonRpcResponse<Value>> {
    let id = request.id.clone();
    let result = async {
        if request.jsonrpc != "2.0" {
            return Err(ProtocolError::InvalidRequest);
        }
        let client = connection.require_prompt_submit()?.clone();
        let params = parse_prompt_params(request.params)?;

        // 先取得/订阅 Actor，再 submit；订阅的 receiver 会暂存首个事件，gate 在响应
        // 入队之前不放行，因此既不会漏掉 PromptAccepted，也不会让 delta 抢在 response 前。
        let input = UserInput::new(params.text)
            .map_err(|_| ProtocolError::InvalidParams("text must not be blank".to_owned()))?;
        // 输入契约优先于环境依赖：即使当前没有 Provider，也必须把空白输入明确报告为
        // invalid params，而不是让客户端误以为补齐配置即可提交无效回合。
        if !runtime.provider_ready() {
            return Err(ProtocolError::RuntimeUnavailable(
                "model provider is not configured".to_owned(),
            ));
        }
        runtime.require_session(&params.session_id)?;
        let handle = runtime
            .supervisor()
            .get_or_start(params.session_id)
            .await
            .map_err(runtime_error)?;
        let gate = event_bridge::spawn(handle.subscribe(), outbound_tx.clone(), cancellation);
        // capability 是连接级快照。每次 submit 前发送 Resume，确保复用的 Actor
        // 不会沿用上一条已关闭连接的审批能力。
        handle.resume(client).await.map_err(runtime_error)?;
        let receipt = handle
            .submit(RuntimeRequestId::new(), input)
            .await
            .map_err(runtime_error)?;
        Ok((
            PromptSubmitResult {
                status: PromptSubmitStatus::Streaming,
                turn_id: receipt.turn_id,
            },
            Some(gate),
        ))
    }
    .await;

    match (id, result) {
        (Some(id), Ok((result, gate))) => {
            // response 与 bridge 的 event 进入同一个 FIFO；先交给调用方发送 response，
            // 随后由 dispatcher 放开 gate，保证 writer 可观察的顺序。
            let response = match serde_json::to_value(result) {
                Ok(result) => JsonRpcResponse::success(id, result),
                Err(error) => {
                    return Some(JsonRpcResponse::failure(
                        id,
                        ProtocolError::Internal(error.to_string()).to_jsonrpc(),
                    ));
                }
            };
            if let Some(gate) = gate {
                // response 必须先进入与 event 共用的 FIFO，再放开 bridge；否则 Tokio
                // 调度可能让已订阅的首个 delta 越过 response。
                if outbound_tx
                    .send(OutboundFrame::critical(&response).ok()?)
                    .await
                    .is_err()
                {
                    return None;
                }
                gate.release();
                return None;
            }
            Some(response)
        }
        (Some(id), Err(error)) => Some(JsonRpcResponse::failure(id, error.to_jsonrpc())),
        (None, Ok((_result, gate))) => {
            if let Some(gate) = gate {
                gate.release();
            }
            None
        }
        (None, Err(_)) => None,
    }
}

/// `prompt.submit` 的 params 必须是对象，和 protocol 的其它方法维持同一输入边界。
fn parse_prompt_params(params: Option<Value>) -> Result<PromptSubmitParams, ProtocolError> {
    let params = params.unwrap_or_else(|| serde_json::json!({}));
    if !params.is_object() {
        return Err(ProtocolError::InvalidParams(
            "params must be an object".to_owned(),
        ));
    }
    serde_json::from_value(params).map_err(|error| ProtocolError::InvalidParams(error.to_string()))
}

/// writer task：stdout 的唯一所有者，保证每帧完整写入并 flush。
async fn writer_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut outbound_rx: mpsc::Receiver<OutboundFrame>,
    _: CancellationToken,
) -> io::Result<()> {
    while let Some(frame) = outbound_rx.recv().await {
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
            // stdin EOF 后不应让仍在运行的 event bridge 持有 outbound sender；取消
            // connection scope 只停止转发，不会向 Actor 投递 interrupt。writer 会继续
            // 排空此前已经入队的 response/event，随后自然结束。
            cancellation.cancel();
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
            // dispatcher 正常结束只意味着 request 流关闭；此时也要停止 bridge，保证
            // outbound channel 能关闭并让唯一 writer 排空后退出。
            cancellation.cancel();
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
