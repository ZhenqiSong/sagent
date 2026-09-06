//! `sagent-rpc` 子进程的单连接客户端。
//!
//! stdout reader、stdin writer 和 pending-response 表都由本模块集中管理：这保证一条
//! response 只会唤醒它的原始请求，且连接故障时所有 waiter 都能同时结束。

use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use sagent_protocol::{
    ApprovalRespondParams, ApprovalRespondResult, ClientHelloCapabilities, ClientHelloParams,
    ClientHelloResult, JsonRpcRequest, JsonRpcResponse, PROTOCOL_VERSION, PromptSubmitParams,
    PromptSubmitResult, RequestId, SessionCreateParams, SessionCreateResult,
    SessionEventsSinceParams, SessionEventsSinceResult, SessionInterruptParams,
    SessionInterruptResult, SessionListParams, SessionListResult, SessionResumeParams,
    SessionResumeResult,
};
use sagent_types::{ClientId, ClientSurface};
use serde_json::Value;
use tokio::{
    io::BufReader,
    process::{Child, Command},
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use super::codec::{ServerFrame, decode_server_frame, read_frame, write_request};
use crate::{
    app::{AppAction, AppState, reduce},
    args::TuiArgs,
};

const OUTBOUND_CAPACITY: usize = 64;
const INBOUND_CAPACITY: usize = 64;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const STDERR_TAIL_LINES: usize = 16;

/// reader 交给连接拥有者的服务端通知。
#[derive(Debug)]
enum Incoming {
    /// `gateway.ready` 等不属于任意 request id 的服务端事件。
    Event(sagent_protocol::JsonRpcEvent<Value>),
    /// transport、进程或协议错误；收到后连接不能继续复用。
    Disconnected(String),
}

/// reader 交给终端循环的非阻塞输入；断线不能被伪装成“暂时没有 event”。
#[derive(Debug)]
pub enum ClientPoll {
    /// 一条服务端通知。
    Event(sagent_protocol::JsonRpcEvent<Value>),
    /// transport 已不可继续使用的稳定错误摘要。
    Disconnected(String),
}

/// 等待 response 的发送端；错误使用字符串以便一次广播给所有 pending waiter。
type PendingResponses =
    Arc<Mutex<HashMap<i64, oneshot::Sender<std::result::Result<Value, String>>>>>;

/// 一个由 TUI 独占的本地 RPC 子进程连接。
///
/// `child` 的所有权留在这里，确保 TUI 退出时能先取消异步读写任务、再关闭 stdin 并
/// 限时等待进程。`--home`/`--profile` 仅在 spawn 期间转换为 argv，不会写入请求参数。
pub struct RpcClient {
    child: Child,
    outbound_tx: mpsc::Sender<JsonRpcRequest>,
    incoming_rx: mpsc::Receiver<Incoming>,
    pending: PendingResponses,
    cancellation: CancellationToken,
    next_id: AtomicI64,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
    stderr_task: JoinHandle<()>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl RpcClient {
    /// 启动受控的 `sagent-rpc` 子进程，并建立 stdin/stdout 的唯一读写任务。
    pub async fn spawn(args: &TuiArgs) -> Result<Self> {
        let mut command = Command::new(&args.rpc_bin);
        command.stdin(std::process::Stdio::piped());
        command.stdout(std::process::Stdio::piped());
        // stderr 绝不能合并到 stdout；后者是严格 NDJSON 协议流，前者仅用于有限诊断。
        command.stderr(std::process::Stdio::piped());
        append_scope_arguments(&mut command, args.home.as_deref(), args.profile.as_deref());
        let mut child = command
            .spawn()
            .with_context(|| format!("无法启动 RPC 子进程 {}", args.rpc_bin.display()))?;
        let stdin = child.stdin.take().context("RPC 子进程没有 stdin")?;
        let stdout = child.stdout.take().context("RPC 子进程没有 stdout")?;
        let stderr = child.stderr.take().context("RPC 子进程没有 stderr")?;

        let cancellation = CancellationToken::new();
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (outbound_tx, outbound_rx) = mpsc::channel(OUTBOUND_CAPACITY);
        let (incoming_tx, incoming_rx) = mpsc::channel(INBOUND_CAPACITY);
        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
        let writer_task = tokio::spawn(writer_loop(
            stdin,
            outbound_rx,
            incoming_tx.clone(),
            pending.clone(),
            cancellation.clone(),
        ));
        let reader_task = tokio::spawn(reader_loop(
            BufReader::new(stdout),
            incoming_tx,
            pending.clone(),
            cancellation.clone(),
        ));
        let stderr_task = tokio::spawn(stderr_loop(BufReader::new(stderr), stderr_tail.clone()));

        Ok(Self {
            child,
            outbound_tx,
            incoming_rx,
            pending,
            cancellation,
            next_id: AtomicI64::new(1),
            reader_task,
            writer_task,
            stderr_task,
            stderr_tail,
        })
    }

    /// 依次等待 `gateway.ready` 并调用 `client.hello`，不能跳过该协议屏障。
    pub async fn handshake(&mut self, state: &mut AppState) -> Result<ClientHelloResult> {
        reduce(state, AppAction::RpcWaitingForReady);
        self.wait_for_ready().await?;
        reduce(state, AppAction::GatewayReady);

        let params = ClientHelloParams {
            protocol_version: PROTOCOL_VERSION,
            client_id: ClientId::new(),
            surface: ClientSurface::Tui,
            capabilities: ClientHelloCapabilities {
                interactive_approval: true,
                supports_stream_edits: false,
            },
        };
        let result: ClientHelloResult = self
            .request("client.hello", serde_json::to_value(params)?)
            .await?;
        reduce(state, AppAction::HelloSucceeded);
        Ok(result)
    }

    /// 发送带单调数字 id 的请求，并等待唯一对应的 response 或连接取消。
    pub async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (response_tx, response_rx) = oneshot::channel();
        self.pending.lock().await.insert(id, response_tx);
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Some(RequestId::Number(id.into())),
            method: method.to_owned(),
            params: Some(params),
        };
        if self.outbound_tx.send(request).await.is_err() {
            self.pending.lock().await.remove(&id);
            bail!("RPC writer 已停止");
        }
        let value = response_rx
            .await
            .map_err(|_| anyhow!("RPC response waiter 被取消"))?
            .map_err(|message| anyhow!(message))?;
        serde_json::from_value(value).context("RPC response result 不符合预期类型")
    }

    /// 调用 `session.list`，返回当前 Profile 的服务端会话摘要页。
    pub async fn list_sessions(&self, params: SessionListParams) -> Result<SessionListResult> {
        self.request("session.list", serde_json::to_value(params)?)
            .await
    }

    /// 调用 `session.create`；会话 id 由 RPC/Store 生成，TUI 不得本地构造它。
    pub async fn create_session(&self, params: SessionCreateParams) -> Result<SessionCreateResult> {
        self.request("session.create", serde_json::to_value(params)?)
            .await
    }

    /// 调用 `session.resume`，获取已应用可见性规则的 transcript 快照。
    pub async fn resume_session(&self, params: SessionResumeParams) -> Result<SessionResumeResult> {
        self.request("session.resume", serde_json::to_value(params)?)
            .await
    }

    /// 调用 `prompt.submit`；成功响应只代表 Turn 已开始，最终文本仍须等待 event/resume。
    pub async fn submit_prompt(&self, params: PromptSubmitParams) -> Result<PromptSubmitResult> {
        self.request("prompt.submit", serde_json::to_value(params)?)
            .await
    }

    /// 请求 Runtime 中断当前 Turn；最终状态必须等待 event。
    pub async fn interrupt_session(
        &self,
        params: SessionInterruptParams,
    ) -> Result<SessionInterruptResult> {
        self.request("session.interrupt", serde_json::to_value(params)?)
            .await
    }

    /// 提交审批决定；TUI 不执行工具，只等待 Runtime 后续事件。
    pub async fn respond_approval(
        &self,
        params: ApprovalRespondParams,
    ) -> Result<ApprovalRespondResult> {
        self.request("approval.respond", serde_json::to_value(params)?)
            .await
    }

    /// 分页读取指定会话断线期间持久化的领域事件。
    pub async fn events_since(
        &self,
        params: SessionEventsSinceParams,
    ) -> Result<SessionEventsSinceResult> {
        self.request("session.events.since", serde_json::to_value(params)?)
            .await
    }

    /// 按受控顺序释放子进程资源；超时后才 kill，避免正常退出时丢弃服务端收尾动作。
    pub async fn shutdown(mut self) {
        self.cancellation.cancel();
        self.reader_task.abort();
        self.writer_task.abort();
        self.stderr_task.abort();
        let _ = self.reader_task.await;
        let _ = self.writer_task.await;
        let _ = self.stderr_task.await;
        fail_pending(&self.pending, "TUI 正在关闭 RPC 连接").await;
        if timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await.is_err() {
            let _ = self.child.kill().await;
            let _ = self.child.wait().await;
        }
    }

    /// 在首个服务端 event 中等待 ready；其它事件保留到后续步骤的统一 action 路由。
    async fn wait_for_ready(&mut self) -> Result<()> {
        timeout(HANDSHAKE_TIMEOUT, async {
            loop {
                match self.incoming_rx.recv().await {
                    Some(Incoming::Event(event)) if event.params.event_type == "gateway.ready" => {
                        return Ok(());
                    }
                    Some(Incoming::Event(_)) => continue,
                    Some(Incoming::Disconnected(message)) => bail!("RPC 连接失败：{message}"),
                    None => bail!("RPC reader 已停止"),
                }
            }
        })
        .await
        .map_err(|_| anyhow!("等待 gateway.ready 超时"))?
    }

    /// 返回最近的子进程 stderr 行，供启动失败与后续状态栏展示使用。
    ///
    /// 缓冲区固定为有限行数，并由子进程独占 reader 填充；它不是协议数据，也不包含
    /// stdout JSON，因而无法影响 response/event 路由。
    pub async fn stderr_summary(&self) -> String {
        self.stderr_tail
            .lock()
            .await
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ")
    }

    /// 非阻塞提取一条服务端 event；终端 tick 使用它避免 RPC reader 与键盘循环竞争 stdout。
    pub fn try_next_event(&mut self) -> Option<ClientPoll> {
        match self.incoming_rx.try_recv() {
            Ok(Incoming::Event(event)) => Some(ClientPoll::Event(event)),
            Ok(Incoming::Disconnected(message)) => Some(ClientPoll::Disconnected(message)),
            Err(_) => None,
        }
    }
}

/// 只将 Profile 作用域放入启动 argv；请求 body 与 UI action 永远没有这两个字段。
fn append_scope_arguments(command: &mut Command, home: Option<&Path>, profile: Option<&str>) {
    if let Some(home) = home {
        command.arg("--home").arg(home);
    }
    if let Some(profile) = profile {
        command.arg("--profile").arg(profile);
    }
}

/// stdin writer 的单一所有者；写失败必须通知所有 waiter，而不是只让当前请求超时。
async fn writer_loop(
    mut stdin: tokio::process::ChildStdin,
    mut outbound_rx: mpsc::Receiver<JsonRpcRequest>,
    incoming_tx: mpsc::Sender<Incoming>,
    pending: PendingResponses,
    cancellation: CancellationToken,
) {
    loop {
        let request = tokio::select! {
            _ = cancellation.cancelled() => return,
            request = outbound_rx.recv() => request,
        };
        let Some(request) = request else {
            return;
        };
        if let Err(error) = write_request(&mut stdin, &request).await {
            disconnect(
                &incoming_tx,
                &pending,
                &cancellation,
                format!("写入 RPC stdin 失败：{error}"),
            )
            .await;
            return;
        }
    }
}

/// stdout reader 的单一所有者；event 与 response 在此分流，避免竞争读取同一字节流。
async fn reader_loop(
    mut stdout: BufReader<tokio::process::ChildStdout>,
    incoming_tx: mpsc::Sender<Incoming>,
    pending: PendingResponses,
    cancellation: CancellationToken,
) {
    loop {
        let frame = tokio::select! {
            _ = cancellation.cancelled() => return,
            frame = read_frame(&mut stdout) => frame,
        };
        let frame = match frame {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                disconnect(
                    &incoming_tx,
                    &pending,
                    &cancellation,
                    "RPC stdout 已关闭".to_owned(),
                )
                .await;
                return;
            }
            Err(error) => {
                disconnect(
                    &incoming_tx,
                    &pending,
                    &cancellation,
                    format!("读取 RPC stdout 失败：{error}"),
                )
                .await;
                return;
            }
        };
        match decode_server_frame(&frame) {
            Ok(ServerFrame::Event(event)) => {
                if incoming_tx.send(Incoming::Event(event)).await.is_err() {
                    return;
                }
            }
            Ok(ServerFrame::Response(response)) => {
                if let Err(message) = route_response(response, &pending).await {
                    disconnect(&incoming_tx, &pending, &cancellation, message).await;
                    return;
                }
            }
            Err(message) => {
                disconnect(&incoming_tx, &pending, &cancellation, message).await;
                return;
            }
        }
    }
}

/// 独立读取 stderr，保留末尾有限诊断行而不阻塞或污染 stdout 协议解析。
async fn stderr_loop(
    mut stderr: BufReader<tokio::process::ChildStderr>,
    tail: Arc<Mutex<VecDeque<String>>>,
) {
    loop {
        // stderr 同样来自不可信子进程，复用有界分帧避免错误日志缺失换行时无界分配。
        let frame = match read_frame(&mut stderr).await {
            Ok(Some(frame)) => frame,
            Ok(None) => return,
            Err(_) => return,
        };
        let diagnostic = String::from_utf8_lossy(&frame);
        let diagnostic = diagnostic.trim();
        if diagnostic.is_empty() {
            continue;
        }
        let mut tail = tail.lock().await;
        if tail.len() == STDERR_TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(diagnostic.to_owned());
    }
}

/// 按数字 request id 路由 response；未知或非数字 id 说明 stdout 已不可信，必须断连。
async fn route_response(
    response: JsonRpcResponse<Value>,
    pending: &PendingResponses,
) -> std::result::Result<(), String> {
    let RequestId::Number(number) = response.id else {
        return Err("RPC response 使用了非数字 request id".to_owned());
    };
    let id = number
        .as_i64()
        .ok_or_else(|| "RPC response request id 超出 i64 范围".to_owned())?;
    let sender = pending
        .lock()
        .await
        .remove(&id)
        .ok_or_else(|| format!("收到未知 RPC response id：{id}"))?;
    let result = match (response.result, response.error) {
        (Some(result), None) => Ok(result),
        (None, Some(error)) => Err(format!("RPC 错误 {}：{}", error.code, error.message)),
        _ => Err("RPC response 同时缺少或同时包含 result/error".to_owned()),
    };
    let _ = sender.send(result);
    Ok(())
}

/// 连接只允许断开一次：取消 I/O task，广播错误，并让 UI 获得稳定 action 来源。
async fn disconnect(
    incoming_tx: &mpsc::Sender<Incoming>,
    pending: &PendingResponses,
    cancellation: &CancellationToken,
    message: String,
) {
    cancellation.cancel();
    fail_pending(pending, &message).await;
    let _ = incoming_tx.send(Incoming::Disconnected(message)).await;
}

/// 移出整个 pending 表再逐个通知，避免发送期间持有异步 mutex。
async fn fail_pending(pending: &PendingResponses, message: &str) {
    let waiters = std::mem::take(&mut *pending.lock().await);
    for (_, sender) in waiters {
        let _ = sender.send(Err(message.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use sagent_protocol::{JsonRpcError, JsonRpcResponse, RequestId};
    use serde_json::json;
    use tokio::sync::{Mutex, oneshot};

    use super::{PendingResponses, route_response};

    #[tokio::test]
    async fn response_is_routed_by_id_even_when_responses_arrive_out_of_order() {
        let pending: PendingResponses =
            std::sync::Arc::new(Mutex::new(std::collections::HashMap::new()));
        let (first_tx, first_rx) = oneshot::channel();
        let (second_tx, second_rx) = oneshot::channel();
        pending.lock().await.insert(1, first_tx);
        pending.lock().await.insert(2, second_tx);

        route_response(
            JsonRpcResponse::success(RequestId::Number(2.into()), json!("second")),
            &pending,
        )
        .await
        .expect("第二个响应应被正确路由");
        route_response(
            JsonRpcResponse::success(RequestId::Number(1.into()), json!("first")),
            &pending,
        )
        .await
        .expect("第一个响应应被正确路由");

        assert_eq!(
            second_rx.await.expect("第二个 waiter 应被唤醒"),
            Ok(json!("second"))
        );
        assert_eq!(
            first_rx.await.expect("第一个 waiter 应被唤醒"),
            Ok(json!("first"))
        );
    }

    #[tokio::test]
    async fn unknown_response_id_marks_stdout_as_untrustworthy() {
        let pending: PendingResponses =
            std::sync::Arc::new(Mutex::new(std::collections::HashMap::new()));
        let response = JsonRpcResponse::failure(
            RequestId::Number(99.into()),
            JsonRpcError {
                code: -32601,
                message: "missing".to_owned(),
                data: None,
            },
        );

        let error = route_response(response, &pending)
            .await
            .expect_err("没有对应 request 的 response 不可静默忽略");
        assert!(error.contains("未知 RPC response id"));
    }
}
