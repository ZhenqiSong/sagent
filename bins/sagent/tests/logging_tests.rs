//! 日志系统端到端测试。
//!
//! 验证 stdout/stderr 通道隔离、敏感数据过滤、结构化日志输出。
//! 所有测试启动 `sagent rpc stdio` 子进程，检查 stderr 日志内容。
//!
//! @author   songzq
//! @created  2025-08-12
//! @change   2025-08-12 初始版本：Phase 0 Step 9 日志隔离测试

use std::io::{BufRead, Read, Write};
use std::process::{Child, Command as StdCommand, Stdio};

/// 启动 sagent stdio server 子进程，捕获 stderr。
fn spawn_server() -> Child {
    StdCommand::new(assert_cmd::cargo::cargo_bin("sagent"))
        .args(["rpc", "stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动 sagent stdio server 失败")
}

/// 发送请求并获取 stdout response。
fn send_request(
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

/// 发送 notification（不期待 response）。
fn send_notification(stdin: &mut std::process::ChildStdin, notification: &str) {
    writeln!(stdin, "{}", notification).expect("写入 stdin 失败");
    stdin.flush().expect("flush stdin 失败");
}

/// 优雅关闭子进程并收集 stderr 输出。
///
/// 先 drop stdin 让子进程收到 EOF，然后 wait 等待退出，最后读取 stderr。
fn shutdown_and_get_stderr(mut child: Child) -> String {
    // drop stdin 让子进程收到 EOF
    drop(child.stdin.take());
    // 等待子进程退出
    let _ = child.wait();
    // 读取 stderr（此时 pipe 已关闭，read_to_string 不会阻塞）
    let mut stderr_output = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut stderr_output);
    }
    stderr_output
}

// ============================================================================
// stdout 协议通道隔离测试
// ============================================================================

#[test]
fn test_stdout_only_contains_valid_json() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout_reader = std::io::BufReader::new(stdout);

    let requests = [
        r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{"value":"hello"}}"#,
        r#"{"jsonrpc":"2.0","id":"2","method":"protocol.describe","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":"3","method":"health.get","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":"4","method":"unknown.method","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":"5","method":"rpc.echo","params":"not-object"}"#,
    ];

    for req in &requests {
        let resp = send_request(&mut stdin, &mut stdout_reader, req);
        assert!(resp.is_object(), "stdout 行不是 JSON object: {}", resp);
        assert_eq!(resp["jsonrpc"], "2.0");
    }

    drop(stdin);
    child.wait().expect("子进程退出失败");
}

// ============================================================================
// stderr 日志内容测试
// ============================================================================

#[test]
fn test_stderr_contains_startup_log() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout_reader = std::io::BufReader::new(stdout);

    let _resp = send_request(
        &mut stdin,
        &mut stdout_reader,
        r#"{"jsonrpc":"2.0","id":"1","method":"health.get","params":{}}"#,
    );

    drop(stdin);
    // 先 wait 再读 stderr，避免阻塞
    let stderr_output = shutdown_and_get_stderr(child);

    assert!(
        stderr_output.contains("sagent stdio JSON-RPC server") || stderr_output.contains("sagent"),
        "stderr 应包含 server 启动日志，实际内容: {}",
        stderr_output
    );
}

#[test]
fn test_stderr_contains_error_log_for_unknown_method() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout_reader = std::io::BufReader::new(stdout);

    let _resp = send_request(
        &mut stdin,
        &mut stdout_reader,
        r#"{"jsonrpc":"2.0","id":"req-err","method":"session.create","params":{}}"#,
    );

    drop(stdin);
    let stderr_output = shutdown_and_get_stderr(child);

    let has_error_log = stderr_output.contains("32601")
        || stderr_output.contains("Method not found")
        || stderr_output.contains("未知方法");
    assert!(
        has_error_log,
        "stderr 应包含 MethodNotFound 错误日志，实际内容: {}",
        stderr_output
    );
}

#[test]
fn test_stderr_contains_error_log_for_parse_error() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout_reader = std::io::BufReader::new(stdout);

    let _resp = send_request(&mut stdin, &mut stdout_reader, "not valid json");

    drop(stdin);
    let stderr_output = shutdown_and_get_stderr(child);

    let has_parse_error = stderr_output.contains("32700")
        || stderr_output.contains("Parse error")
        || stderr_output.contains("JSON 解析失败");
    assert!(
        has_parse_error,
        "stderr 应包含 ParseError 日志，实际内容: {}",
        stderr_output
    );
}

#[test]
fn test_stderr_contains_request_id_in_log() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout_reader = std::io::BufReader::new(stdout);

    let _resp = send_request(
        &mut stdin,
        &mut stdout_reader,
        r#"{"jsonrpc":"2.0","id":"custom-req-id-123","method":"rpc.echo","params":{"x":1}}"#,
    );

    drop(stdin);
    let stderr_output = shutdown_and_get_stderr(child);

    let has_request_id = stderr_output.contains("custom-req-id-123");
    assert!(
        has_request_id,
        "stderr 应包含 request_id 'custom-req-id-123'，实际内容: {}",
        stderr_output
    );
}

// ============================================================================
// 敏感数据过滤测试
// ============================================================================

#[test]
fn test_sensitive_params_not_in_stderr() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout_reader = std::io::BufReader::new(stdout);

    let _resp = send_request(
        &mut stdin,
        &mut stdout_reader,
        r#"{"jsonrpc":"2.0","id":"sensitive-test","method":"rpc.echo","params":{"token":"my-secret-token-value","name":"test"}}"#,
    );

    drop(stdin);
    let stderr_output = shutdown_and_get_stderr(child);

    // stderr 中不应出现真实的敏感值
    assert!(
        !stderr_output.contains("my-secret-token-value"),
        "stderr 不应包含真实 token 值，但找到了 'my-secret-token-value'"
    );
}

#[test]
fn test_sensitive_api_key_not_in_stderr() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout_reader = std::io::BufReader::new(stdout);

    let _resp = send_request(
        &mut stdin,
        &mut stdout_reader,
        r#"{"jsonrpc":"2.0","id":"key-test","method":"rpc.echo","params":{"api_key":"sk-abc123-secret","model":"claude-3"}}"#,
    );

    drop(stdin);
    let stderr_output = shutdown_and_get_stderr(child);

    assert!(
        !stderr_output.contains("sk-abc123-secret"),
        "stderr 不应包含真实 API key 值，但找到了 'sk-abc123-secret'"
    );
}

// ============================================================================
// notification 测试
// ============================================================================

#[test]
fn test_notification_produces_no_stdout_response() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout_reader = std::io::BufReader::new(stdout);

    send_notification(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"rpc.echo","params":{"value":"notify"}}"#,
    );

    let resp = send_request(
        &mut stdin,
        &mut stdout_reader,
        r#"{"jsonrpc":"2.0","id":"check","method":"rpc.echo","params":{"value":"after"}}"#,
    );

    assert_eq!(resp["id"], "check");
    assert_eq!(resp["result"]["value"], "after");

    drop(stdin);
    let stderr_output = shutdown_and_get_stderr(child);

    let has_notification_log =
        stderr_output.contains("notification") || stderr_output.contains("不返回 response");
    assert!(
        has_notification_log,
        "stderr 应包含 notification 日志，实际内容: {}",
        stderr_output
    );
}

// ============================================================================
// 幂等性测试
// ============================================================================

#[test]
fn test_logging_init_is_idempotent_in_process() {
    sagent_api::logging::init();
    sagent_api::logging::init();
    sagent_api::logging::init_with_level("debug");
    sagent_api::logging::init_with_level("error");
    assert!(sagent_api::logging::is_initialized());
}

// ============================================================================
// EOF / 正常退出测试
// ============================================================================

#[test]
fn test_server_exits_cleanly_on_stdin_eof() {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout_reader = std::io::BufReader::new(stdout);

    let resp = send_request(
        &mut stdin,
        &mut stdout_reader,
        r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{"x":1}}"#,
    );
    assert_eq!(resp["result"]["x"], 1);

    drop(stdin);
    let stderr_output = shutdown_and_get_stderr(child);

    let has_exit_log = stderr_output.contains("EOF")
        || stderr_output.contains("已停止")
        || stderr_output.contains("stopped");
    assert!(
        has_exit_log,
        "stderr 应包含正常退出日志，实际内容: {}",
        stderr_output
    );
}

#[test]
fn test_debug_log_level_increases_stderr_not_stdout() {
    let mut child = StdCommand::new(assert_cmd::cargo::cargo_bin("sagent"))
        .args(["rpc", "stdio"])
        .env("RUST_LOG", "debug")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动 sagent stdio server 失败");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout_reader = std::io::BufReader::new(stdout);

    let resp = send_request(
        &mut stdin,
        &mut stdout_reader,
        r#"{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{"value":"test"}}"#,
    );

    assert!(resp.is_object());
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["result"]["value"], "test");

    drop(stdin);
    let stderr_output = shutdown_and_get_stderr(child);

    let has_debug = stderr_output.contains("DEBUG") || stderr_output.len() > 100;
    assert!(
        has_debug,
        "RUST_LOG=debug 时 stderr 应包含更多日志，stderr 长度: {}",
        stderr_output.len()
    );
}
