//! `sagent-tui` 的真实 PTY 黑盒测试。
//!
//! 这些测试不调用 TUI 的内部 reducer，也不把 RPC 替换成 fake：每个场景都启动真实
//! `sagent-tui`，由它再启动真实 `sagent-rpc`，Provider 则连接 loopback Mock SSE。PTY
//! 是必要的，因为 raw mode 和 alternate screen 只有在伪终端中才会走真实路径。

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use sagent_provider::mock::{MockSseChunk, MockSseServer};
use sagent_store::{MessageQuery, Store};
use sagent_types::SessionId;
use sysinfo::{Pid, ProcessesToUpdate, System};

const SCREEN_WAIT: Duration = Duration::from_secs(15);
const INTERRUPT_WAIT: Duration = Duration::from_secs(8);
static NEXT_HOME: AtomicU64 = AtomicU64::new(0);

/// 从 integration-test 可执行文件位置推导 workspace target/debug 下的兄弟 binary。
///
/// Cargo 只为当前 package 的 binary 注入 `CARGO_BIN_EXE_*`；TUI 黑盒仍需启动同一
/// workspace 的 `sagent-rpc`，因此使用 target 目录的稳定相对布局而不硬编码仓库路径。
fn binary_path(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("测试进程应有可执行文件路径");
    path.pop();
    path.pop();
    path.push(name);
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path
}

/// 从当前测试进程隔离出一个 Profile 根和 workspace，避免读写开发机用户目录。
fn fixture_home(name: &str) -> PathBuf {
    let counter = NEXT_HOME.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时钟应可读取")
        .as_nanos();
    let home = std::env::temp_dir().join(format!(
        "sagent-tui-blackbox-{name}-{}-{counter}-{nanos}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(home.join("workspace")).expect("应能创建临时 workspace");
    home
}

/// 写入只含 fixture endpoint/key 的 Profile 配置；workspace 使用 Profile 相对路径。
fn configure_home(home: &Path, endpoint: &str) {
    fs::write(
        home.join("config.yaml"),
        format!(
            "provider: openai-compatible\nmodel: tui-fixture\nbase_url: {endpoint}\napi_key_env: SAGENT_TUI_FIXTURE_KEY\nworkspace: workspace\n"
        ),
    )
    .expect("应能写入临时 Provider 配置");
    fs::write(home.join(".env"), "SAGENT_TUI_FIXTURE_KEY=fixture-key\n")
        .expect("应能写入临时 fixture 凭据");
}

/// 返回当前宿主 shell 的审批 fixture；命令保持破坏性分类，但只触碰 fixture 路径。
#[cfg(windows)]
fn approval_command() -> &'static str {
    "del /s /q __sagent_missing_fixture__.txt & echo approved > approval-marker.txt"
}

/// macOS/Linux 使用 POSIX shell 语法，确保同一黑盒在原生 Unix host 可执行。
#[cfg(not(windows))]
fn approval_command() -> &'static str {
    "rm -rf __sagent_missing_fixture__.txt && printf approved > approval-marker.txt"
}

/// 生成携带平台命令的 OpenAI-compatible tool-call SSE fixture，避免手写 JSON 转义。
fn terminal_tool_call_sse() -> String {
    let arguments = serde_json::json!({"command": approval_command()}).to_string();
    let call = serde_json::json!({
        "id": "tool",
        "choices": [{
            "delta": {"tool_calls": [{
                "index": 0,
                "id": "call-terminal",
                "function": {"name": "terminal", "arguments": arguments}
            }]}
        }]
    });
    let finish = serde_json::json!({
        "id": "tool",
        "choices": [{"finish_reason": "tool_calls"}]
    });
    format!("data: {call}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

/// PTY reader 的有界消息；解析与等待只在测试进程中进行，不改变 TUI 的协议流。
enum ReaderMessage {
    Bytes(Vec<u8>),
    Closed,
}

/// 驱动真实 TUI 的小型 PTY harness。
struct PtySession {
    // ConPTY 的 inner handle 由 MasterPty 持有；reader/writer 是其文件描述符副本，不能
    // 单独维持伪终端生命周期，因此必须把 master 保存到测试会话结束。
    _master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    reader_rx: Receiver<ReaderMessage>,
    output: Vec<u8>,
    closed: bool,
    cursor_query_replied: bool,
}

impl PtySession {
    /// 启动 TUI，并显式传递临时 home 与 RPC binary，避免继承默认 Profile。
    fn spawn(home: &Path) -> Self {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("应能创建 PTY");
        let portable_pty::PtyPair { slave, master } = pair;
        let mut command = CommandBuilder::new(binary_path("sagent-tui"));
        command.arg("--home");
        command.arg(home);
        command.arg("--rpc-bin");
        command.arg(binary_path("sagent-rpc"));
        let child = slave
            .spawn_command(command)
            .expect("应能启动真实 sagent-tui");
        let reader = master.try_clone_reader().expect("应能克隆 PTY reader");
        let writer = master.take_writer().expect("应能取得 PTY writer");
        let (reader_tx, reader_rx) = mpsc::channel();
        thread::spawn(move || read_pty(reader, reader_tx));

        Self {
            _master: master,
            child,
            writer,
            reader_rx,
            output: Vec::new(),
            closed: false,
            cursor_query_replied: false,
        }
    }

    /// 返回 TUI 的进程 id，用于在断线场景精确定位其直接 RPC 子进程。
    fn process_id(&self) -> u32 {
        self.child.process_id().expect("PTY child 应有进程 id")
    }

    /// 向 raw-mode TUI 发送一组真实键盘字节。
    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("应能写入 PTY");
        self.writer.flush().expect("应能刷新 PTY 输入");
    }

    /// 等待一个屏幕标记，并返回它在当前输出中的字节偏移。
    fn wait_for(&mut self, marker: &str, timeout: Duration) -> usize {
        self.wait_for_after(0, marker, timeout)
    }

    /// 只在指定偏移之后查找标记，避免把初次连接的屏幕重绘误认为重连完成。
    fn wait_for_after(&mut self, offset: usize, marker: &str, timeout: Duration) -> usize {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(position) = screen_text(&self.output[offset..]).find(marker) {
                return offset + position;
            }
            if Instant::now() >= deadline {
                let child_status = self
                    .child
                    .try_wait()
                    .ok()
                    .flatten()
                    .map(|status| format!(" child_status={status:?}"))
                    .unwrap_or_default();
                let tail = screen_text(&self.output)
                    .chars()
                    .rev()
                    .take(800)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>();
                panic!(
                    "PTY 未在 {:?} 内看到 {:?}；{}原始字节={} {:?}，输出尾部：{}",
                    timeout,
                    marker,
                    child_status,
                    self.output.len(),
                    self.output,
                    tail
                );
            }
            self.receive_until(deadline);
        }
    }

    /// 等待 TUI 正常退出；退出码由 PTY 实现转换为统一 success 判断。
    fn wait_success(&mut self) {
        let status = self.child.wait().expect("应能等待 TUI 退出");
        assert!(status.success(), "TUI 非正常退出：{status:?}");
        self.closed = true;
    }

    /// 当前屏幕输出长度，用作下一次等待的边界。
    fn output_len(&self) -> usize {
        self.output.len()
    }

    fn receive_until(&mut self, deadline: Instant) {
        if self.closed {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let wait = remaining.min(Duration::from_millis(100));
        match self.reader_rx.recv_timeout(wait) {
            Ok(ReaderMessage::Bytes(bytes)) => {
                self.output.extend(bytes);
                self.respond_to_cursor_query();
            }
            Ok(ReaderMessage::Closed) => self.closed = true,
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {}
        }
    }

    /// Crossterm 查询光标位置时，真实终端模拟器会回写 CPR；PTY harness 必须完成这个
    /// 最小终端协议，否则 TUI 会在首次 draw 前一直等待，屏幕不会产生可断言文本。
    fn respond_to_cursor_query(&mut self) {
        if self.cursor_query_replied || !self.output.windows(4).any(|window| window == b"\x1b[6n") {
            return;
        }
        self.writer
            .write_all(b"\x1b[1;1R")
            .expect("应能回写 CPR 响应");
        self.writer.flush().expect("应能刷新 CPR 响应");
        self.cursor_query_replied = true;
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// 独立 reader 线程只做字节搬运；PTY close 后发送 Closed 让等待器产生明确诊断。
fn read_pty(mut reader: Box<dyn Read + Send>, sender: mpsc::Sender<ReaderMessage>) {
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                let _ = sender.send(ReaderMessage::Closed);
                return;
            }
            Ok(count) => {
                if sender
                    .send(ReaderMessage::Bytes(buffer[..count].to_vec()))
                    .is_err()
                {
                    return;
                }
            }
            Err(_) => {
                let _ = sender.send(ReaderMessage::Closed);
                return;
            }
        }
    }
}

/// Ratatui 输出含 ANSI 控制序列；这里只去除 CSI/OSC 外壳，保留可断言的可见文本。
fn screen_text(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    let mut visible = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' && chars.next() == Some('\\') {
                            break;
                        }
                    }
                }
                Some(_) | None => {}
            }
        } else if character != '\r' {
            visible.push(character);
        }
    }
    visible
}

/// 返回临时 Profile 的唯一会话，并确认数据库中的消息投影可读取。
fn only_session(home: &Path) -> (SessionId, Vec<sagent_types::StoredMessage>) {
    let store = Store::open_readonly(&home.join("state.db")).expect("应能只读打开临时数据库");
    let sessions = store.list_sessions(10, 0).expect("应能读取会话列表");
    assert_eq!(sessions.len(), 1, "fixture 应只有一个会话");
    let session_id = sessions[0].id.clone();
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取 transcript");
    (session_id, messages)
}

/// 等待 actor 把中断/失败写入 Store；轮询的是持久化事实而不是任意 wall-clock sleep。
async fn wait_for_no_running_turn(home: &Path, session_id: &SessionId) {
    let deadline = Instant::now() + INTERRUPT_WAIT;
    loop {
        if let Ok(store) = Store::open_readonly(&home.join("state.db"))
            && store
                .get_running_turn(session_id)
                .expect("running Turn 查询应稳定")
                .is_none()
        {
            return;
        }
        assert!(Instant::now() < deadline, "中断终态未在限定时间内持久化");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// 只杀掉指定 TUI 进程的直接 sagent-rpc 子进程，模拟 transport 断线而非用户退出。
fn kill_rpc_child(tui_pid: u32) {
    let parent = Pid::from_u32(tui_pid);
    let deadline = Instant::now() + SCREEN_WAIT;
    let mut system = System::new();
    loop {
        system.refresh_processes(ProcessesToUpdate::All, true);
        if let Some(process) = system.processes().values().find(|process| {
            process.parent() == Some(parent)
                && process
                    .name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .starts_with("sagent-rpc")
        }) {
            let pid = process.pid().as_u32();
            if cfg!(windows) {
                // Windows 的子进程可能继承 stdout/stderr 管道；taskkill /T /F 同时
                // 终止其进程树并关闭句柄，确保 TUI 的 reader 能收到 EOF 触发重连。
                let status = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .expect("应能调用 taskkill 终止 RPC 子进程");
                assert!(status.success(), "taskkill 必须成功终止 RPC 子进程");
            } else {
                assert!(process.kill(), "应能终止精确定位的 sagent-rpc 子进程");
            }
            return;
        }
        assert!(Instant::now() < deadline, "未找到 TUI 的 sagent-rpc 子进程");
        thread::sleep(Duration::from_millis(25));
    }
}

/// 真实 TUI 完成空 Profile 的 list/create/resume/submit/stream/complete 生命周期。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tui_blackbox_completes_create_submit_and_resume_flow() {
    let home = fixture_home("complete");
    let server = MockSseServer::spawn(vec![MockSseChunk::text(include_str!(
        "../../sagent-provider/tests/fixtures/provider/normal_text.sse"
    ))])
    .await
    .expect("应能启动 Mock SSE");
    configure_home(&home, &server.url());

    let mut tui = PtySession::spawn(&home);
    tui.wait_for("暂无会话", SCREEN_WAIT);
    tui.send(b"n");
    tui.wait_for("此会话暂无可见消息", SCREEN_WAIT);
    tui.send(b"hello\r");
    tui.wait_for("你好，Sagent", SCREEN_WAIT);
    tui.send(b"q");
    tui.wait_success();

    server.wait().await.expect("Mock SSE 应完成一次请求");
    let (_session_id, messages) = only_session(&home);
    assert_eq!(
        messages
            .iter()
            .map(|message| message.role.as_str())
            .collect::<Vec<_>>(),
        ["user", "assistant"],
        "TUI 完成后 transcript 必须由服务端持久化快照提供"
    );
    assert_eq!(
        messages.last().map(|message| message.content.as_str()),
        Some("你好，Sagent")
    );
    fs::remove_dir_all(home).expect("应能清理 complete fixture");
}

/// 危险 terminal 在 TUI 审批前不执行，Once 后才创建 marker 并进入 Provider 第二轮。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tui_blackbox_requires_approval_before_terminal_and_completes() {
    let home = fixture_home("approval");
    let first = terminal_tool_call_sse();
    let second = include_str!("../../sagent-provider/tests/fixtures/provider/normal_text.sse");
    let server = MockSseServer::spawn_sequence(vec![
        vec![MockSseChunk::text(first)],
        vec![MockSseChunk::text(second)],
    ])
    .await
    .expect("应能启动多轮 Mock SSE");
    configure_home(&home, &server.url());

    let mut tui = PtySession::spawn(&home);
    tui.wait_for("暂无会话", SCREEN_WAIT);
    tui.send(b"n");
    tui.wait_for("此会话暂无可见消息", SCREEN_WAIT);
    tui.send(b"run\r");
    tui.wait_for("工具审批", SCREEN_WAIT);
    assert!(
        !home.join("workspace").join("approval-marker.txt").exists(),
        "approval.requested 到达后 terminal 仍不得启动"
    );
    tui.send(b"1");
    tui.wait_for("你好，Sagent", SCREEN_WAIT);
    tui.send(b"q");
    tui.wait_success();

    assert!(
        home.join("workspace").join("approval-marker.txt").is_file(),
        "Once 响应后 terminal 才能在固定 workspace 执行"
    );
    server.wait().await.expect("两轮 Mock SSE 都应完成");
    let (_session_id, messages) = only_session(&home);
    assert!(messages.iter().any(|message| message.role == "tool"));
    assert_eq!(
        messages.last().map(|message| message.content.as_str()),
        Some("你好，Sagent")
    );
    fs::remove_dir_all(home).expect("应能清理 approval fixture");
}

/// Ctrl-C 通过真实 RPC interrupt 收口 Turn，不伪造 assistant 最终消息或重复执行。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tui_blackbox_ctrl_c_persists_interrupted_without_assistant() {
    let home = fixture_home("interrupt");
    let server = MockSseServer::spawn(vec![MockSseChunk::delayed(
        include_str!("../../sagent-provider/tests/fixtures/provider/normal_text.sse"),
        Duration::from_secs(10),
    )])
    .await
    .expect("应能启动慢速 Mock SSE");
    configure_home(&home, &server.url());

    let mut tui = PtySession::spawn(&home);
    tui.wait_for("暂无会话", SCREEN_WAIT);
    tui.send(b"n");
    tui.wait_for("此会话暂无可见消息", SCREEN_WAIT);
    tui.send(b"cancel\r");
    tui.wait_for("assistant（生成中）", SCREEN_WAIT);
    let (session_id, _) = only_session(&home);
    tui.send(&[0x03]);
    wait_for_no_running_turn(&home, &session_id).await;
    tui.send(b"q");
    tui.wait_success();

    let (_session_id, messages) = only_session(&home);
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.role == "assistant")
            .count(),
        0
    );
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.role == "user")
            .count(),
        1
    );
    fs::remove_dir_all(home).expect("应能清理 interrupt fixture");
}

/// RPC 子进程被外部终止后，TUI 必须重连同一 Profile，并且 resume 不重复 transcript。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tui_blackbox_reconnects_and_keeps_transcript_unique() {
    let home = fixture_home("reconnect");
    let server = MockSseServer::spawn(vec![MockSseChunk::delayed(
        include_str!("../../sagent-provider/tests/fixtures/provider/normal_text.sse"),
        Duration::from_secs(10),
    )])
    .await
    .expect("应能启动慢速 Mock SSE");
    configure_home(&home, &server.url());

    let mut tui = PtySession::spawn(&home);
    tui.wait_for("暂无会话", SCREEN_WAIT);
    tui.send(b"n");
    tui.wait_for("此会话暂无可见消息", SCREEN_WAIT);
    tui.send(b"reconnect\r");
    tui.wait_for("assistant（生成中）", SCREEN_WAIT);
    let (session_id, before) = only_session(&home);
    let tui_pid = tui.process_id();
    let offset = tui.output_len();
    kill_rpc_child(tui_pid);
    // 重连握手发生在下一次 TerminalGuard 建立前，WaitingForReady 不会被绘制；
    // 以重连完成后重新绘制的 Connected 状态作为黑盒可见同步点。
    tui.wait_for_after(offset, "已连接 RPC", SCREEN_WAIT);

    let (_session_id, after) = only_session(&home);
    assert_eq!(
        before.len(),
        after.len(),
        "重连 resume 不得复制任何 transcript 消息"
    );
    assert_eq!(
        after
            .iter()
            .filter(|message| message.role == "user")
            .count(),
        1
    );
    assert!(
        Store::open_readonly(&home.join("state.db"))
            .expect("应能只读打开数据库")
            .get_running_turn(&session_id)
            .expect("running Turn 查询应稳定")
            .is_some(),
        "仅断线不会伪造 Turn 终态，后续 Runtime 恢复负责 fail-closed 收口"
    );
    tui.send(b"q");
    tui.wait_success();
    fs::remove_dir_all(home).expect("应能清理 reconnect fixture");
}
