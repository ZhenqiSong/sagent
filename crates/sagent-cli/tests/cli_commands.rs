//! 通过真实二进制验证 CLI 的参数、stdout 和 profile 隔离。
//!
//! 作者：SongZQ

use std::{
    fs,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;

static NEXT_TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn test_home(name: &str) -> std::path::PathBuf {
    let sequence = NEXT_TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "sagent-cli-integration-{name}-{}-{sequence}",
        std::process::id()
    ))
}

fn run(home: &std::path::Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sagent-cli"))
        .arg("--home")
        .arg(home)
        .args(arguments)
        .output()
        .expect("应能启动 sagent-cli 二进制")
}

fn parse_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "命令应成功：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "成功命令不应向 stderr 写入内容");
    serde_json::from_slice(&output.stdout).expect("stdout 应为完整 JSON")
}

#[test]
fn json_commands_preserve_profile_isolation() {
    let home = test_home("profiles");
    let _ = fs::remove_dir_all(&home);

    let created_profile = parse_json(&run(
        &home,
        &["--format", "json", "profile", "create", "coder"],
    ));
    assert_eq!(
        created_profile["path"],
        home.join("profiles").join("coder").display().to_string()
    );

    let default_session = parse_json(&run(
        &home,
        &[
            "--format",
            "json",
            "session",
            "create",
            "--title",
            "默认 profile 会话",
        ],
    ));

    let created_session = parse_json(&run(
        &home,
        &[
            "--format",
            "json",
            "--profile",
            "coder",
            "session",
            "create",
            "--title",
            "集成测试会话",
            "--model",
            "test-model",
        ],
    ));
    let session_id = created_session["session_id"]
        .as_str()
        .expect("创建结果必须包含会话 ID");

    let coder_sessions = parse_json(&run(
        &home,
        &["--format", "json", "--profile", "coder", "session", "list"],
    ));
    assert_eq!(coder_sessions.as_array().expect("列表应为数组").len(), 1);
    assert_eq!(coder_sessions[0]["id"], session_id);

    let default_sessions = parse_json(&run(&home, &["--format", "json", "session", "list"]));
    assert_eq!(default_sessions.as_array().expect("列表应为数组").len(), 1);
    assert_eq!(
        default_sessions[0]["id"], default_session["session_id"],
        "默认 profile 不应读取 coder 的会话"
    );
    fs::remove_dir_all(home).expect("应能清理测试目录");
}

#[test]
fn invalid_command_returns_non_zero_status_and_diagnostic_on_stderr() {
    let home = test_home("error");
    let _ = fs::remove_dir_all(&home);

    let output = run(&home, &["profile", "create", "default"]);

    assert_eq!(output.status.code(), Some(2), "参数错误必须返回退出码 2");
    assert!(output.stdout.is_empty(), "失败命令不应污染 stdout");
    assert!(String::from_utf8_lossy(&output.stderr).contains("无需创建"));
}

#[test]
fn missing_state_database_returns_not_found_exit_code() {
    let home = test_home("missing-db");
    let _ = fs::remove_dir_all(&home);

    let output = run(&home, &["--format", "json", "session", "list"]);

    assert_eq!(
        output.status.code(),
        Some(3),
        "缺失 state.db 必须返回退出码 3"
    );
    assert!(output.stdout.is_empty(), "失败命令不应污染 stdout");
    assert!(String::from_utf8_lossy(&output.stderr).contains("state.db 不存在"));
}
