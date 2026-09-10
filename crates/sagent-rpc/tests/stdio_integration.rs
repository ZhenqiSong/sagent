//! `sagent-rpc` 的真实子进程 NDJSON 契约测试。

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use sagent_provider::mock::{MockSseChunk, MockSseServer};
use sagent_store::{MessageQuery, NewDaemonEvent, NewMessage, NewSession, Store};
use sagent_types::SessionId;
use serde_json::{Value, json};

fn test_home(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("sagent-rpc-{name}-{}", std::process::id()))
}

fn remove(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

fn create_fixture(home: &Path) -> PathBuf {
    fs::create_dir_all(home).expect("应能创建临时 home");
    let database = home.join("state.db");
    let visible_id = SessionId::new("visible-session");
    let archived_id = SessionId::new("archived-session");
    let mut store = Store::open_readwrite(&database).expect("应能创建 fixture 数据库");

    store
        .create_session(&NewSession {
            id: visible_id.clone(),
            source: Some("tui".to_owned()),
            model: Some("test-model".to_owned()),
            title: Some("可见会话".to_owned()),
            started_at: "2026-09-01T10:00:00Z".to_owned(),
        })
        .expect("应能创建可见会话");
    for (role, content, timestamp) in [
        ("user", "第一条提问", "2026-09-01T10:01:00Z"),
        ("assistant", "第一条回答", "2026-09-01T10:02:00Z"),
    ] {
        store
            .append_message(&NewMessage::new(
                visible_id.clone(),
                role,
                content,
                timestamp,
            ))
            .expect("应能写入可见消息");
    }
    // 这些均是可恢复事实；真实 stream delta 不进入 fixture，也不应进入 events.since。
    for (event_type, created_at) in [
        ("turn.started", "2026-09-01T10:01:00Z"),
        ("tool.completed", "2026-09-01T10:01:30Z"),
        ("turn.completed", "2026-09-01T10:02:00Z"),
    ] {
        store
            .append_event(&NewDaemonEvent {
                session_id: visible_id.clone(),
                turn_id: None,
                event_type: event_type.to_owned(),
                payload: json!({"fixture": true}),
                created_at: created_at.to_owned(),
            })
            .expect("应能写入可恢复 fixture 事件");
    }
    store
        .create_session(&NewSession {
            id: archived_id.clone(),
            source: Some("cli".to_owned()),
            model: None,
            title: Some("归档会话".to_owned()),
            started_at: "2026-09-01T09:00:00Z".to_owned(),
        })
        .expect("应能创建归档会话");
    store
        .set_session_archived(&archived_id, true, "2026-09-01T09:01:00Z")
        .expect("应能归档 fixture 会话");
    drop(store);
    database
}

fn run_rpc(home: &Path, profile: Option<&str>, input: &str) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sagent-rpc"));
    command.args(["--home", home.to_str().expect("临时路径必须是 UTF-8")]);
    if let Some(profile) = profile {
        command.args(["--profile", profile]);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("应能启动 sagent-rpc");
    child
        .stdin
        .as_mut()
        .expect("应有 stdin")
        .write_all(input.as_bytes())
        .expect("应能写入 RPC 请求");
    child.wait_with_output().expect("应能等待 RPC 退出")
}

/// 保持 stdin 打开，直到收到指定终态事件。
///
/// EOF 代表客户端主动断开，transport 会停止 event bridge；流式 E2E 因而不能复用
/// `run_rpc` 的“写完即关闭 stdin”模型，必须模拟一个仍在观察事件的真实客户端。
fn run_rpc_until_event(home: &Path, input: &str, terminal_event: &str) -> (Vec<Value>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sagent-rpc"))
        .args(["--home", home.to_str().expect("临时路径必须是 UTF-8")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("应能启动 sagent-rpc");
    let mut stdin = child.stdin.take().expect("应有 stdin");
    stdin
        .write_all(input.as_bytes())
        .expect("应能写入 RPC 请求");
    stdin.flush().expect("应能刷新 RPC 请求");

    let stdout = child.stdout.take().expect("应有 stdout");
    let mut stdout = BufReader::new(stdout);
    let mut frames = Vec::new();
    loop {
        let mut line = String::new();
        let count = stdout.read_line(&mut line).expect("应能读取 NDJSON 输出");
        assert_ne!(count, 0, "终态事件前 stdout 不应 EOF");
        let frame: Value = serde_json::from_str(line.trim_end()).expect("每帧必须是 JSON");
        let terminal = frame["params"]["type"] == terminal_event;
        frames.push(frame);
        if terminal {
            break;
        }
    }
    drop(stdin);
    let output = child.wait_with_output().expect("应能等待 RPC 退出");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (frames, String::from_utf8_lossy(&output.stderr).into_owned())
}

fn output_frames(output: &[u8]) -> Vec<Value> {
    std::str::from_utf8(output)
        .expect("stdout 必须是 UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout 每一行必须是 JSON"))
        .collect()
}

#[test]
fn stdio_protocol_reads_sessions_without_writing_database() {
    let home = test_home("full-contract");
    remove(&home);
    let database = create_fixture(&home);
    let bytes_before = fs::read(&database).expect("应能读取 fixture 数据库");
    let size_before = fs::metadata(&database).expect("应能读取文件元数据").len();
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session.list\",\"params\":{}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session.list\",\"params\":{\"include_archived\":true}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session.resume\",\"params\":{\"session_id\":\"visible-session\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"session.resume\",\"params\":{\"session_id\":\"missing\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"unknown.method\",\"params\":{}}\n",
        "not-json\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"gateway.ping\",\"params\":{}}\n"
    );

    let output = run_rpc(&home, None, input);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "stderr 不应包含诊断输出");
    let frames = output_frames(&output.stdout);
    assert_eq!(frames.len(), 7, "notification 不应产生响应");
    assert_eq!(frames[0]["method"], "event");
    assert_eq!(frames[0]["params"]["type"], "gateway.ready");
    assert_eq!(
        frames[1]["result"]["sessions"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        frames[2]["result"]["sessions"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(frames[3]["result"]["session"]["id"], "visible-session");
    assert_eq!(
        frames[3]["result"]["messages"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(frames[4]["error"]["code"], json!(-32004));
    assert_eq!(frames[5]["error"]["code"], json!(-32601));
    assert_eq!(frames[6]["error"]["code"], json!(-32700));

    assert_eq!(
        fs::metadata(&database).expect("应能读取元数据").len(),
        size_before
    );
    assert_eq!(fs::read(&database).expect("应能读取数据库"), bytes_before);
    remove(&home);
}

#[test]
fn stdio_protocol_negotiates_client_hello_before_read_only_requests() {
    let home = test_home("client-hello");
    remove(&home);
    create_fixture(&home);
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"client.hello\",\"params\":{",
        "\"protocol_version\":1,",
        "\"client_id\":\"550e8400-e29b-41d4-a716-446655440000\",",
        "\"surface\":\"tui\",",
        "\"capabilities\":{\"interactive_approval\":true,\"supports_stream_edits\":false}",
        "}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session.list\",\"params\":{}}\n"
    );

    let output = run_rpc(&home, None, input);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frames = output_frames(&output.stdout);

    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0]["params"]["type"], "gateway.ready");
    assert_eq!(frames[1]["result"]["protocol_version"], 1);
    assert_eq!(
        frames[1]["result"]["features"],
        json!([
            "gateway.ping",
            "config.read",
            "session.list",
            "session.resume",
            "client.hello",
            "session.create",
            "prompt.submit",
            "session.interrupt",
            "approval.respond",
            "session.events.since"
        ])
    );
    assert_eq!(
        frames[1]["result"]["capabilities"]["interactive_approval"],
        true
    );
    assert_eq!(frames[2]["result"]["sessions"][0]["id"], "visible-session");
    remove(&home);
}

#[test]
fn config_read_returns_profile_summary_without_credential_or_endpoint_fields() {
    // 这里经过真实 daemon 进程验证启动期快照，确保 transport 既不会读取 .env 返回，
    // 也不会把 endpoint 这类部署拓扑泄露到 GUI/RPC 客户端。
    let home = test_home("config-read");
    remove(&home);
    create_fixture(&home);
    fs::write(
        home.join("config.yaml"),
        "provider: local\nmodel: local-model\nbase_url: http://private.example/v1\napi_key_env: PRIVATE_KEY\nproviders:\n  backup:\n    api: http://backup.example/v1\nfuture_field: enabled\n",
    )
    .expect("应能写入 Profile 配置");
    fs::write(home.join(".env"), "PRIVATE_KEY=must-not-leak\n").expect("应能写入测试凭据");

    let output = run_rpc(
        &home,
        None,
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"config.read\",\"params\":{}}\n",
    );
    assert!(output.status.success(), "daemon 应能读取公开配置摘要");
    let frames = output_frames(&output.stdout);
    assert_eq!(frames[1]["result"]["profile"], "default");
    assert_eq!(frames[1]["result"]["provider"], "local");
    assert_eq!(frames[1]["result"]["model"], "local-model");
    assert_eq!(frames[1]["result"]["provider_names"], json!(["backup"]));
    assert_eq!(
        frames[1]["result"]["unknown_fields"],
        json!(["future_field"])
    );
    let response = frames[1]["result"].to_string();
    assert!(!response.contains("private.example"));
    assert!(!response.contains("PRIVATE_KEY"));
    assert!(!response.contains("must-not-leak"));
    remove(&home);
}

#[test]
fn missing_database_is_initialized_for_the_runtime_daemon() {
    let home = test_home("missing-db");
    remove(&home);
    fs::create_dir_all(&home).expect("应能创建空 home");

    let output = run_rpc(
        &home,
        None,
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session.list\",\"params\":{}}\n",
    );
    assert!(
        output.status.success(),
        "首次运行应初始化 Sagent 自有数据库，stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frames = output_frames(&output.stdout);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1]["result"]["sessions"], json!([]));
    assert!(home.join("state.db").is_file(), "bootstrap 应创建 state.db");
    remove(&home);
}

#[test]
fn named_profile_reads_its_own_database() {
    let root = test_home("named-profile");
    remove(&root);
    let profile_home = root.join("profiles").join("coder");
    create_fixture(&profile_home);
    let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session.list\",\"params\":{}}\n";

    let output = run_rpc(&root, Some("coder"), input);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frames = output_frames(&output.stdout);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1]["result"]["sessions"][0]["id"], "visible-session");
    remove(&root);
}

#[test]
fn interactive_methods_are_gated_by_connection_hello_and_capability() {
    let home = test_home("connection-gate");
    remove(&home);
    create_fixture(&home);
    let input = concat!(
        // 尚未握手时，交互方法必须在解析业务参数前被拒绝。
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"prompt.submit\",\"params\":{}}\n",
        // 错误版本不能建立连接状态；后续请求仍然需要 hello。
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"client.hello\",\"params\":{\"protocol_version\":999,\"client_id\":\"550e8400-e29b-41d4-a716-446655440000\",\"surface\":\"tui\",\"capabilities\":{\"interactive_approval\":true,\"supports_stream_edits\":false}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"prompt.submit\",\"params\":{}}\n",
        // hello 成功但未声明审批能力时，错误应从 handshake 升级为 capability denied。
        "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"client.hello\",\"params\":{\"protocol_version\":1,\"client_id\":\"550e8400-e29b-41d4-a716-446655440000\",\"surface\":\"tui\",\"capabilities\":{\"interactive_approval\":false,\"supports_stream_edits\":false}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"approval.respond\",\"params\":{}}\n",
    );

    let output = run_rpc(&home, None, input);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frames = output_frames(&output.stdout);

    assert_eq!(frames.len(), 6, "ready 加五个带 id 的请求应返回六帧");
    assert_eq!(frames[1]["error"]["code"], json!(-32006));
    assert_eq!(frames[2]["error"]["code"], json!(-32007));
    assert_eq!(frames[3]["error"]["code"], json!(-32006));
    assert_eq!(frames[4]["result"]["protocol_version"], json!(1));
    assert_eq!(frames[5]["error"]["code"], json!(-32008));
    remove(&home);
}

#[test]
fn prompt_submit_validates_input_and_hides_unconfigured_provider_details() {
    let home = test_home("prompt-unconfigured");
    remove(&home);
    create_fixture(&home);
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"client.hello\",\"params\":{\"protocol_version\":1,\"client_id\":\"550e8400-e29b-41d4-a716-446655440000\",\"surface\":\"tui\",\"capabilities\":{\"interactive_approval\":false,\"supports_stream_edits\":false}}}\n",
        // 空白文本属于协议输入错误，不能被“尚未配置 Provider”掩盖。
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"prompt.submit\",\"params\":{\"session_id\":\"visible-session\",\"text\":\"   \"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"prompt.submit\",\"params\":{\"session_id\":\"visible-session\",\"text\":\"hello\"}}\n",
    );

    let output = run_rpc(&home, None, input);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frames = output_frames(&output.stdout);

    assert_eq!(frames[2]["error"]["code"], json!(-32602));
    assert_eq!(frames[3]["error"]["code"], json!(-32011));
    assert_eq!(frames[3]["error"]["message"], json!("runtime unavailable"));
    assert!(frames[3]["error"].get("data").is_none());
    remove(&home);
}

#[test]
fn control_requests_require_the_right_connection_state_and_an_active_turn() {
    let home = test_home("control-gate");
    remove(&home);
    create_fixture(&home);
    let input = concat!(
        // hello 前不会因为 session_id 不存在或无 active actor 而泄露运行时状态。
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session.interrupt\",\"params\":{\"session_id\":\"visible-session\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"client.hello\",\"params\":{\"protocol_version\":1,\"client_id\":\"550e8400-e29b-41d4-a716-446655440000\",\"surface\":\"tui\",\"capabilities\":{\"interactive_approval\":false,\"supports_stream_edits\":false}}}\n",
        // 已持久化但未运行的会话不能被取消请求隐式启动。
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session.interrupt\",\"params\":{\"session_id\":\"visible-session\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"approval.respond\",\"params\":{\"session_id\":\"visible-session\",\"turn_id\":\"turn-1\",\"approval_id\":\"approval-1\",\"decision\":\"deny\"}}\n",
    );

    let output = run_rpc(&home, None, input);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frames = output_frames(&output.stdout);

    assert_eq!(frames[1]["error"]["code"], json!(-32006));
    assert_eq!(frames[3]["error"]["code"], json!(-32010));
    assert_eq!(
        frames[3]["error"]["data"],
        json!({"session_id": "visible-session", "turn_id": null})
    );
    assert_eq!(frames[4]["error"]["code"], json!(-32008));
    remove(&home);
}

#[test]
fn events_since_replays_only_current_session_facts_in_sequence_order() {
    let home = test_home("events-since");
    remove(&home);
    create_fixture(&home);
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"client.hello\",\"params\":{\"protocol_version\":1,\"client_id\":\"550e8400-e29b-41d4-a716-446655440000\",\"surface\":\"tui\",\"capabilities\":{\"interactive_approval\":false,\"supports_stream_edits\":false}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session.events.since\",\"params\":{\"session_id\":\"visible-session\",\"after_sequence\":0,\"limit\":2}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session.events.since\",\"params\":{\"session_id\":\"visible-session\",\"after_sequence\":2,\"limit\":999}}\n",
    );

    let output = run_rpc(&home, None, input);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frames = output_frames(&output.stdout);

    let first_page = &frames[2]["result"];
    assert_eq!(first_page["events"].as_array().map(Vec::len), Some(2));
    assert_eq!(first_page["events"][0]["sequence"], json!(1));
    assert_eq!(first_page["events"][1]["sequence"], json!(2));
    assert_eq!(first_page["events"][0]["event_type"], json!("turn.started"));
    assert_eq!(first_page["has_more"], json!(true));
    assert_eq!(first_page["latest_sequence"], json!(3));

    let second_page = &frames[3]["result"];
    assert_eq!(second_page["events"].as_array().map(Vec::len), Some(1));
    assert_eq!(second_page["events"][0]["sequence"], json!(3));
    assert_eq!(second_page["has_more"], json!(false));
    remove(&home);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mock_sse_drives_real_rpc_stream_and_persists_final_message() {
    let home = test_home("mock-sse-e2e");
    remove(&home);
    fs::create_dir_all(&home).expect("应能创建临时 Profile");
    let server = MockSseServer::spawn(vec![MockSseChunk::text(include_str!(
        "../../sagent-provider/tests/fixtures/provider/normal_text.sse"
    ))])
    .await
    .expect("应能启动本地 Mock SSE");
    // 仅使用 fixture key；resolver 从当前临时 Profile 读取，不依赖开发机环境。
    fs::write(
        home.join("config.yaml"),
        format!(
            "provider: openai-compatible\nmodel: mock-model\nbase_url: {}\napi_key_env: SAGE_TEST_KEY\n",
            server.url()
        ),
    )
    .expect("应能写入临时 provider 配置");
    fs::write(home.join(".env"), "SAGE_TEST_KEY=fixture-key\n").expect("应能写入临时凭据");
    // session id 在 create 响应后才知道；为在同一 stdin 批次中提交 prompt，测试先
    // 在同一 Profile 写入会话，实际 create → submit 串联由客户端状态机负责。
    let session_id = SessionId::new("mock-sse-session");
    let mut store = Store::open_readwrite(&home.join("state.db")).expect("应能打开状态库");
    store
        .create_session(&NewSession {
            id: session_id.clone(),
            source: Some("rpc-test".to_owned()),
            model: Some("mock-model".to_owned()),
            title: None,
            started_at: "2026-09-06T00:00:00Z".to_owned(),
        })
        .expect("应能创建 E2E 会话");
    drop(store);
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"client.hello\",\"params\":{\"protocol_version\":1,\"client_id\":\"550e8400-e29b-41d4-a716-446655440000\",\"surface\":\"tui\",\"capabilities\":{\"interactive_approval\":false,\"supports_stream_edits\":false}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"prompt.submit\",\"params\":{\"session_id\":\"mock-sse-session\",\"text\":\"你好\"}}\n",
    )
    .to_owned();
    let home_for_process = home.clone();
    let (frames, stderr) = tokio::task::spawn_blocking(move || {
        run_rpc_until_event(&home_for_process, &input, "turn.completed")
    })
    .await
    .expect("阻塞子进程任务不应 panic");

    assert!(stderr.is_empty(), "stderr 不应泄露 fixture key 或诊断");
    let submit = frames
        .iter()
        .position(|frame| frame["id"] == json!(2))
        .expect("submit 必须有立即响应");
    let first_delta = frames
        .iter()
        .position(|frame| frame["params"]["type"] == "message.delta")
        .expect("Mock SSE 必须产生 delta");
    assert!(submit < first_delta, "submit response 必须先于首个 delta");
    assert!(
        frames
            .iter()
            .any(|frame| frame["params"]["type"] == "message.complete")
    );

    server.wait().await.expect("Mock SSE 应服务一次请求");
    let store = Store::open_readonly(&home.join("state.db")).expect("应能重开状态库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取持久化消息");
    assert_eq!(
        messages.last().map(|message| message.content.as_str()),
        Some("你好，Sagent")
    );
    remove(&home);
}

#[test]
fn hello_then_session_create_persists_an_empty_rpc_session() {
    let home = test_home("session-create");
    remove(&home);
    fs::create_dir_all(&home).expect("应能创建临时 home");
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"client.hello\",\"params\":{\"protocol_version\":1,\"client_id\":\"550e8400-e29b-41d4-a716-446655440000\",\"surface\":\"tui\",\"capabilities\":{\"interactive_approval\":false,\"supports_stream_edits\":false}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session.create\",\"params\":{\"title\":\"RPC 空会话\"}}\n",
    );

    // 先执行 hello/create，随后由同一 Profile 的 Store 验证没有因创建空会话启动 Actor。
    let output = run_rpc(&home, None, input);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frames = output_frames(&output.stdout);
    assert_eq!(frames.len(), 3);
    let session_id = frames[2]["result"]["session_id"]
        .as_str()
        .expect("create 应返回 session id");
    assert_eq!(frames[2]["result"]["session"]["source"], json!("rpc"));
    assert_eq!(frames[2]["result"]["session"]["title"], json!("RPC 空会话"));

    let store = Store::open_readonly(&home.join("state.db")).expect("应能重新只读打开数据库");
    let session = store
        .get_session(&SessionId::new(session_id))
        .expect("应能读取刚创建的会话")
        .expect("会话应存在");
    assert_eq!(session.source.as_deref(), Some("rpc"));
    assert_eq!(session.message_count, 0, "创建空会话不能写入 user message");
    assert!(
        store
            .get_messages_for_display(&SessionId::new(session_id), &Default::default())
            .expect("应能读取空会话消息")
            .is_empty(),
        "创建空会话不能启动 Actor 或生成消息"
    );
    remove(&home);
}
