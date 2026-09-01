//! `sagent-rpc` 的真实子进程 NDJSON 契约测试。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use sagent_store::{NewMessage, NewSession, Store};
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
fn missing_database_fails_without_creating_state_file() {
    let home = test_home("missing-db");
    remove(&home);
    fs::create_dir_all(&home).expect("应能创建空 home");

    let output = run_rpc(&home, None, "");
    assert!(!output.status.success(), "缺失数据库必须令进程失败");
    assert!(output.stdout.is_empty(), "启动失败时 stdout 不能产生协议帧");
    assert!(String::from_utf8_lossy(&output.stderr).contains("state.db"));
    assert!(!home.join("state.db").exists(), "只读 RPC 不能创建数据库");
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
