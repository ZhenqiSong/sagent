//! stdio JSON-RPC echo server 端到端一致性测试。
//!
//! 启动 `sagent rpc stdio` 子进程，通过 stdin/stdout 验证完整的请求-响应周期。
//! 覆盖成功路径、错误路径、notification、连续请求和边界条件。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 7 端到端一致性测试

use std::io::{BufRead, Write};
use std::process::{Child, Command as StdCommand, Stdio};

/// 启动 sagent stdio server 子进程。
fn spawn_server() -> Child {
    StdCommand::new(assert_cmd::cargo::cargo_bin("sagent"))
        .args(["rpc", "stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动 sagent stdio server 失败")
}

/// 发送一行并读取一行响应。
fn send_and_recv(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut std::io::BufReader<std::process::ChildStdout>,
    request: &str,
) -> serde_json::Value {
    writeln!(stdin, "{}", request).expect("写入 stdin 失败");
    stdin.flush().expect("flush stdin 失败");

    let mut line = String::new();
    stdout.read_line(&mut line).expect("读取 stdout 失败");
    serde_json::from_str(line.trim()).expect("解析 response 失败")
}

/// 发送请求并获取 response。
fn send_request(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut std::io::BufReader<std::process::ChildStdout>,
    request: &str,
) -> serde_json::Value {
    send_and_recv(stdin, stdout, request)
}

// ============================================================================
// 基本功能测试
// ============================================================================

#[test]
fn test_rpc_echo_returns_params() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{"value":"hello"}}"#,
    );
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], "1");
    assert_eq!(resp["result"]["value"], "hello");
    assert!(resp.get("error").is_none());

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

#[test]
fn test_protocol_describe_returns_version() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"1","method":"protocol.describe","params":{}}"#,
    );
    assert_eq!(resp["result"]["protocol"], "sagent.rpc");
    assert_eq!(resp["result"]["version"], 1);
    assert!(resp["result"]["features"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("rpc.echo")));

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

#[test]
fn test_health_get_returns_ok() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"health-1","method":"health.get","params":{}}"#,
    );
    assert_eq!(resp["result"]["status"], "ok");

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

// ============================================================================
// 连续请求测试
// ============================================================================

#[test]
fn test_two_consecutive_requests_output_two_lines_in_order() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    let resp1 = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{"a":1}}"#,
    );
    let resp2 = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"2","method":"rpc.echo","params":{"b":2}}"#,
    );

    assert_eq!(resp1["id"], "1");
    assert_eq!(resp1["result"]["a"], 1);
    assert_eq!(resp2["id"], "2");
    assert_eq!(resp2["result"]["b"], 2);

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

// ============================================================================
// Notification 测试
// ============================================================================

#[test]
fn test_notification_does_not_produce_response() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    // 发送 notification（无 id）
    let notification = r#"{"jsonrpc":"2.0","method":"rpc.echo","params":{"value":"notify"}}"#;
    writeln!(stdin, "{}", notification).unwrap();
    stdin.flush().unwrap();

    // 发送一个正常的 request
    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"check","method":"rpc.echo","params":{"value":"after"}}"#,
    );

    // notification 不应产生 response，所以收到的第一个 response 应该是 id="check"
    assert_eq!(resp["id"], "check");
    assert_eq!(resp["result"]["value"], "after");

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

// ============================================================================
// 错误路径测试
// ============================================================================

#[test]
fn test_invalid_json_returns_parse_error() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    let resp = send_request(&mut stdin, &mut stdout, "not valid json");
    assert_eq!(resp["error"]["code"], -32700);

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

#[test]
fn test_missing_method_returns_invalid_request() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"1","params":{}}"#,
    );
    assert_eq!(resp["error"]["code"], -32600);

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

#[test]
fn test_unknown_method_returns_method_not_found() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"1","method":"session.create","params":{}}"#,
    );
    assert_eq!(resp["error"]["code"], -32601);

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

#[test]
fn test_params_is_array_returns_invalid_params() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":["a","b"]}"#,
    );
    assert_eq!(resp["error"]["code"], -32602);

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

#[test]
fn test_wrong_jsonrpc_version_returns_invalid_request() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"1.0","id":"1","method":"rpc.echo","params":{}}"#,
    );
    assert_eq!(resp["error"]["code"], -32600);

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

#[test]
fn test_unknown_envelope_field_returns_invalid_request() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{},"extra":true}"#,
    );
    assert_eq!(resp["error"]["code"], -32600);
    assert_eq!(resp["id"], "1");

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

#[test]
fn test_method_too_long_returns_payload_too_large() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);
    let method = "a".repeat(257);
    let request = format!(
        r#"{{"jsonrpc":"2.0","id":"1","method":"{}","params":{{}}}}"#,
        method
    );

    let resp = send_request(&mut stdin, &mut stdout, &request);
    assert_eq!(resp["error"]["code"], -32003);

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

#[test]
fn test_oversized_line_returns_payload_too_large_and_continues() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);
    let oversized = format!("{{\"padding\":\"{}\"}}", "a".repeat(1024 * 1024));

    let oversized_response = send_request(&mut stdin, &mut stdout, &oversized);
    assert_eq!(oversized_response["error"]["code"], -32003);

    let response = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"after","method":"rpc.echo","params":{"ok":true}}"#,
    );
    assert_eq!(response["id"], "after");
    assert_eq!(response["result"]["ok"], true);

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

// ============================================================================
// Response 不变量测试
// ============================================================================

#[test]
fn test_response_contains_only_result_or_error() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    // 成功 response 只有 result，没有 error
    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{"x":1}}"#,
    );
    assert!(resp.get("result").is_some());
    assert!(resp.get("error").is_none());

    // 错误 response 只有 error，没有 result
    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"2","method":"unknown.method","params":{}}"#,
    );
    assert!(resp.get("error").is_some());
    assert!(resp.get("result").is_none());

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

#[test]
fn test_stdout_each_line_is_valid_json() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    let requests = [
        r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{"a":1}}"#,
        r#"{"jsonrpc":"2.0","id":"2","method":"rpc.echo","params":{"b":2}}"#,
        r#"{"jsonrpc":"2.0","id":"3","method":"rpc.echo","params":{"c":3}}"#,
    ];

    for req in &requests {
        let resp = send_request(&mut stdin, &mut stdout, req);
        assert!(resp.is_object(), "stdout 每行都应是 JSON object");
    }

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

// ============================================================================
// 空行和边界测试
// ============================================================================

#[test]
fn test_empty_lines_are_ignored() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    // 发送空行
    writeln!(stdin).unwrap();
    writeln!(stdin, "   ").unwrap();
    stdin.flush().unwrap();

    // 发送正常请求
    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{"value":"ok"}}"#,
    );
    assert_eq!(resp["result"]["value"], "ok");

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

#[test]
fn test_number_id_is_supported() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":42,"method":"rpc.echo","params":{"x":1}}"#,
    );
    assert_eq!(resp["id"], 42);
    assert_eq!(resp["result"]["x"], 1);

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

#[test]
fn test_error_response_retains_request_id() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout = std::io::BufReader::new(stdout);

    let resp = send_request(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":"my-custom-id","method":"unknown.method","params":{}}"#,
    );
    assert_eq!(resp["error"]["code"], -32601);
    assert_eq!(resp["id"], "my-custom-id");

    drop(stdin);
    child.wait().expect("子进程退出失败");
}
