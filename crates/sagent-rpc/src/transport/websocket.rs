//! 仅限本机的 WebSocket JSON-RPC transport。
//!
//! 本模块只负责 WebSocket 握手、逐 message 的 JSON 编解码和连接生灭；所有 method
//! dispatch、capability gate、Actor 控制与事件顺序仍复用 stdio 的连接内核。

use std::{io, net::SocketAddr, sync::Arc};

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use sagent_protocol::{DispatchService, JsonRpcError, JsonRpcRequest, JsonRpcResponse, RequestId};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

use crate::{
    connection::ConnectionState,
    runtime_bootstrap::RuntimeBootstrap,
    stdio::{self, InboundFrame, MAX_FRAME_BYTES, OutboundFrame},
};

const REQUEST_CHANNEL_CAPACITY: usize = 64;
const OUTBOUND_CHANNEL_CAPACITY: usize = 256;

/// 在指定 loopback 地址上持续接收 WebSocket 连接。
///
/// 每个连接各自拥有 `ConnectionState` 与只读 SQLite 连接，所以 hello capability、请求
/// 背压和断线都不会串到其它客户端；共享的 Supervisor 则让同一 Profile 的重连能够继续
/// 访问既有 Actor。
pub async fn run(address: SocketAddr, bootstrap: Arc<RuntimeBootstrap>) -> Result<()> {
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("绑定 WebSocket 地址 {address} 失败"))?;
    let bound_address = listener
        .local_addr()
        .context("读取 WebSocket 监听地址失败")?;
    eprintln!("sagent-rpc: WebSocket listening on ws://{bound_address}");

    loop {
        let (stream, peer) = listener.accept().await.context("接受 WebSocket 连接失败")?;
        let service = match bootstrap.open_service() {
            Ok(service) => service,
            Err(error) => {
                // 一个连接的只读 Store 打开失败不能结束其它本地客户端；连接尚未升级，
                // 只能记录 server 端诊断并丢弃它，不能伪造不完整的 JSON-RPC 信封。
                eprintln!("sagent-rpc: 拒绝 WebSocket {peer}: {error:#}");
                continue;
            }
        };
        tokio::spawn(async move {
            if let Err(error) = serve_connection(stream, service).await {
                // WebSocket 连接错误与 stdio EOF 不等价：它仅影响这个客户端，不能把
                // listener 或同 Profile 的其它 Actor 一并取消。
                eprintln!("sagent-rpc: WebSocket {peer} 已关闭: {error}");
            }
        });
    }
}

/// 运行一条已经接受的 TCP 连接，直到客户端关闭或唯一 writer 失败。
async fn serve_connection<S>(stream: TcpStream, service: S) -> io::Result<()>
where
    S: DispatchService + Send + 'static,
{
    let websocket = accept_async(stream).await.map_err(websocket_error)?;
    let (writer, reader) = websocket.split();
    let (request_tx, request_rx) = mpsc::channel(REQUEST_CHANNEL_CAPACITY);
    let (outbound_tx, outbound_rx) = mpsc::channel(OUTBOUND_CHANNEL_CAPACITY);
    outbound_tx
        .send(stdio::ready_frame()?)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "WebSocket writer stopped"))?;

    let cancellation = CancellationToken::new();
    let reader_task = tokio::spawn(reader_loop(reader, request_tx, cancellation.child_token()));
    let dispatcher_task = tokio::spawn(stdio::dispatcher_loop(
        request_rx,
        outbound_tx,
        service,
        ConnectionState::new(),
        cancellation.child_token(),
    ));
    let writer_task = tokio::spawn(writer_loop(writer, outbound_rx, cancellation.child_token()));

    let mut reader_task = reader_task;
    let mut dispatcher_task = dispatcher_task;
    let mut writer_task = writer_task;
    tokio::select! {
        result = &mut writer_task => {
            let result = join_result(result, "WebSocket writer")?;
            cancellation.cancel();
            reader_task.abort();
            dispatcher_task.abort();
            result
        }
        result = &mut reader_task => {
            let result = join_result(result, "WebSocket reader")?;
            if result.is_err() {
                cancellation.cancel();
                dispatcher_task.abort();
                writer_task.abort();
                return result;
            }
            let result = join_result(dispatcher_task.await, "WebSocket dispatcher")?;
            cancellation.cancel();
            if result.is_err() {
                writer_task.abort();
                return result;
            }
            join_result(writer_task.await, "WebSocket writer")?
        }
        result = &mut dispatcher_task => {
            let result = join_result(result, "WebSocket dispatcher")?;
            cancellation.cancel();
            reader_task.abort();
            if result.is_err() {
                writer_task.abort();
                return result;
            }
            join_result(writer_task.await, "WebSocket writer")?
        }
    }
}

/// 将 WebSocket text/binary message 转成与 stdio 相同的 dispatcher 输入。
async fn reader_loop(
    mut reader: futures_util::stream::SplitStream<WebSocketStream<TcpStream>>,
    request_tx: mpsc::Sender<InboundFrame>,
    cancellation: CancellationToken,
) -> io::Result<()> {
    loop {
        let message = tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            message = reader.next() => message,
        };
        let Some(message) = message else {
            return Ok(());
        };
        let message = match message {
            Ok(message) => message,
            // 对端正常 close 和网络连接断开都只结束当前连接，不是 server 级错误。
            Err(
                tokio_tungstenite::tungstenite::Error::ConnectionClosed
                | tokio_tungstenite::tungstenite::Error::AlreadyClosed,
            ) => return Ok(()),
            Err(error) => return Err(websocket_error(error)),
        };
        let Some(inbound) = decode_message(message) else {
            continue;
        };
        if request_tx.send(inbound).await.is_err() {
            return Ok(());
        }
    }
}

/// 限制单条 WebSocket message，避免 WebSocket 比 stdio 获得更宽松的内存边界。
fn decode_message(message: Message) -> Option<InboundFrame> {
    match message {
        Message::Text(text) => Some(decode_json(text.as_bytes())),
        Message::Binary(bytes) => Some(if bytes.len() > MAX_FRAME_BYTES {
            oversized_frame()
        } else {
            invalid_frame("binary WebSocket frames are not supported")
        }),
        Message::Close(_) => None,
        // ping/pong 属于 WebSocket 控制流，并非 JSON-RPC 请求；不向 dispatcher 暴露。
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => None,
    }
}

/// 解析 JSON 文本，错误信封与 stdio 保持同一 JSON-RPC 错误分类。
fn decode_json(bytes: &[u8]) -> InboundFrame {
    if bytes.len() > MAX_FRAME_BYTES {
        return oversized_frame();
    }
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

fn oversized_frame() -> InboundFrame {
    invalid_frame(&format!("request frame exceeds {MAX_FRAME_BYTES} bytes"))
}

fn invalid_frame(message: &str) -> InboundFrame {
    InboundFrame::Response(JsonRpcResponse::failure(
        RequestId::Null,
        sagent_protocol::ProtocolError::InvalidParams(message.to_owned()).to_jsonrpc(),
    ))
}

/// WebSocket 的唯一 writer；每个 RPC frame 恰好映射为一个 text message。
async fn writer_loop(
    mut writer: futures_util::stream::SplitSink<WebSocketStream<TcpStream>, Message>,
    mut outbound_rx: mpsc::Receiver<OutboundFrame>,
    cancellation: CancellationToken,
) -> io::Result<()> {
    while let Some(frame) = tokio::select! {
        _ = cancellation.cancelled() => None,
        frame = outbound_rx.recv() => frame,
    } {
        writer
            .send(Message::Text(frame.text().to_owned().into()))
            .await
            .map_err(websocket_error)?;
    }
    Ok(())
}

fn websocket_error(error: tokio_tungstenite::tungstenite::Error) -> io::Error {
    io::Error::other(format!("WebSocket transport error: {error}"))
}

fn join_result(
    result: Result<io::Result<()>, tokio::task::JoinError>,
    task: &str,
) -> io::Result<io::Result<()>> {
    result.map_err(|error| io::Error::other(format!("{task} task failed: {error}")))
}

#[cfg(test)]
mod tests {
    use futures_util::{SinkExt, StreamExt};
    use sagent_protocol::{
        ConfigReadParams, ConfigReadResult, ConfigReadService, GatewayPingResult, GatewayService,
        SessionCreateParams, SessionCreateResult, SessionCreateService, SessionListParams,
        SessionListResult, SessionReadService, SessionResumeParams, SessionResumeResult,
    };
    use tokio::net::TcpListener;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    use super::serve_connection;

    struct FakeService;

    impl GatewayService for FakeService {
        fn ping(&self) -> GatewayPingResult {
            GatewayPingResult {
                ok: true,
                protocol_version: 1,
            }
        }
    }

    impl ConfigReadService for FakeService {
        fn read_config(
            &self,
            _: &ConfigReadParams,
        ) -> Result<ConfigReadResult, sagent_protocol::ProtocolError> {
            Ok(ConfigReadResult {
                profile: "default".to_owned(),
                provider: None,
                model: None,
                provider_names: vec![],
                unknown_fields: vec![],
            })
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
                "missing".to_owned(),
            ))
        }
    }

    impl SessionCreateService for FakeService {
        fn create_session(
            &self,
            _: &SessionCreateParams,
        ) -> Result<SessionCreateResult, sagent_protocol::ProtocolError> {
            Err(sagent_protocol::ProtocolError::Internal(
                "fixture does not create sessions".to_owned(),
            ))
        }
    }

    #[tokio::test]
    async fn websocket_reuses_ready_and_jsonrpc_dispatch_contract() {
        // 使用真实 loopback TCP 与 WebSocket 升级，验证 WebSocket 仅替换帧边界而非复制
        // hello/dispatch 逻辑；ready 必须先于客户端的首个 response。
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("应能绑定 loopback");
        let address = listener.local_addr().expect("应能读取监听地址");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("应能接受测试连接");
            serve_connection(stream, FakeService).await
        });

        let (mut client, _) = connect_async(format!("ws://{address}"))
            .await
            .expect("应能升级 WebSocket");
        let ready = client
            .next()
            .await
            .expect("应有 ready message")
            .expect("ready 应有效");
        let ready: serde_json::Value =
            serde_json::from_str(ready.into_text().expect("ready 必须是 text").as_ref())
                .expect("ready 必须是 JSON");
        assert_eq!(ready["params"]["type"], "gateway.ready");

        client
            .send(Message::Text(
                "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"gateway.ping\",\"params\":{}}".into(),
            ))
            .await
            .expect("应能发送 ping");
        let response = client
            .next()
            .await
            .expect("应有 response")
            .expect("response 应有效");
        let response: serde_json::Value =
            serde_json::from_str(response.into_text().expect("response 必须是 text").as_ref())
                .expect("response 必须是 JSON");
        assert_eq!(response["id"], 7);
        assert_eq!(response["result"]["ok"], true);

        client.close(None).await.expect("应能关闭客户端");
        server
            .await
            .expect("server task 不应 panic")
            .expect("正常 close 不应报错");
    }

    #[test]
    fn websocket_rejects_binary_and_oversized_messages_without_dispatching_them() {
        let binary =
            super::decode_message(Message::Binary(vec![1, 2].into())).expect("binary 应产生错误帧");
        let oversized =
            super::decode_message(Message::Text("x".repeat(super::MAX_FRAME_BYTES + 1).into()))
                .expect("超大 text 应产生错误帧");
        let frame = match binary {
            super::InboundFrame::Response(response) => response,
            _ => panic!("binary 不应进入 dispatcher"),
        };
        let oversized = match oversized {
            super::InboundFrame::Response(response) => response,
            _ => panic!("超大帧不应进入 dispatcher"),
        };
        assert_eq!(frame.error.expect("应有错误").code, -32602);
        assert_eq!(oversized.error.expect("应有错误").code, -32602);
    }
}
