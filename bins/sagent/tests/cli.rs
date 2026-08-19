//! 基础 Session CLI 端到端测试。
//!
//! 通过多个 CLI 子进程验证 Runtime 持久化、JSON 输出和稳定错误行为。
//!
//! @author   songzq
//! @created  2026-08-18
//! @change   2026-08-18 初始版本：Phase 1 Step 8 CLI 测试

use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

fn test_home() -> std::path::PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "sagent-cli-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn run(home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(assert_cmd::cargo::cargo_bin("sagent"))
        .args(args)
        .env("SAGENT_HOME", home)
        .output()
        .expect("启动 sagent CLI 失败")
}

#[test]
fn session_lifecycle_persists_across_processes() {
    let home = test_home();
    let output = run(&home, &["session", "create", "--title", "demo", "--json"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert!(
        output.stderr.is_empty(),
        "CLI 输出污染 stderr: {:?}",
        output.stderr
    );
    let created: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let session_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["title"], "demo");

    let output = run(&home, &["session", "list", "--json"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let listed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], session_id);

    let output = run(&home, &["session", "get", &session_id, "--json"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let fetched: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(fetched["session"]["id"], session_id);
    assert_eq!(fetched["messages"].as_array().unwrap().len(), 0);

    let output = run(&home, &["session", "resume", &session_id, "--json"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let resumed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(resumed["session"]["id"], session_id);
}

#[test]
fn missing_session_has_nonzero_exit_and_stable_error() {
    let output = run(&test_home(), &["session", "get", "missing", "--json"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Session 不存在"), "stderr: {stderr}");
}

#[test]
fn health_and_protocol_json_are_parseable() {
    let home = test_home();
    let output = run(&home, &["health", "--json"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let health: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(health["status"], "ok");

    let output = run(&home, &["protocol", "describe", "--json"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let protocol: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(protocol["protocol"], "sagent.rpc");
    assert!(protocol["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|feature| { feature == "session.create" }));
}
