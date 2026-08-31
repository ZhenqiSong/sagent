//! 通过真实二进制验证 CLI 的参数、stdout 和 profile 隔离。
//!
//! 作者：SongZQ

use std::{
    fs,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

use sagent_store::{NewMessage, Store};
use sagent_types::SessionId;
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

#[test]
fn list_include_archived_exposes_archived_session_only_when_requested() {
    let home = test_home("archived");
    let _ = fs::remove_dir_all(&home);
    let created = parse_json(&run(
        &home,
        &["--format", "json", "session", "create", "--title", "待归档"],
    ));
    let session_id = created["session_id"]
        .as_str()
        .expect("创建结果必须包含会话 ID");

    let renamed = parse_json(&run(
        &home,
        &[
            "--format",
            "json",
            "session",
            "rename",
            session_id,
            "新的标题",
        ],
    ));
    assert_eq!(renamed["operation"], "rename");
    assert_eq!(renamed["title"], "新的标题");
    assert_eq!(renamed["changed"], true);

    let archived = parse_json(&run(
        &home,
        &["--format", "json", "session", "archive", session_id],
    ));
    assert_eq!(archived["operation"], "archive");
    assert_eq!(archived["changed"], true);

    let default_list = parse_json(&run(&home, &["--format", "json", "session", "list"]));
    assert!(default_list.as_array().expect("列表应为数组").is_empty());

    let archive_list = parse_json(&run(
        &home,
        &["--format", "json", "session", "list", "--include-archived"],
    ));
    assert_eq!(archive_list.as_array().expect("列表应为数组").len(), 1);
    assert_eq!(archive_list[0]["id"], session_id);

    let unarchived = parse_json(&run(
        &home,
        &["--format", "json", "session", "unarchive", session_id],
    ));
    assert_eq!(unarchived["operation"], "unarchive");
    assert_eq!(unarchived["changed"], true);

    let finished = parse_json(&run(
        &home,
        &[
            "--format",
            "json",
            "session",
            "finish",
            session_id,
            "--reason",
            "user_done",
        ],
    ));
    assert_eq!(finished["operation"], "finish");
    assert_eq!(finished["reason"], "user_done");

    let detail = parse_json(&run(
        &home,
        &["--format", "json", "session", "show", session_id],
    ));
    assert_eq!(detail["session"]["title"], "新的标题");
    assert_eq!(detail["session"]["end_reason"], "user_done");
    assert!(detail["session"]["ended_at"].is_string());
    fs::remove_dir_all(home).expect("应能清理测试目录");
}

#[test]
fn rewind_hides_active_messages_but_keeps_auditable_history() {
    let home = test_home("rewind");
    let _ = fs::remove_dir_all(&home);
    let created = parse_json(&run(
        &home,
        &[
            "--format",
            "json",
            "session",
            "create",
            "--title",
            "回退测试",
        ],
    ));
    let session_id = created["session_id"]
        .as_str()
        .expect("创建结果必须包含会话 ID");
    let session_id_type = SessionId::new(session_id);
    let mut store = Store::open_readwrite(&home.join("state.db")).expect("应能打开 state.db");
    let user_message_id = store
        .append_message(&NewMessage::new(
            session_id_type.clone(),
            "user",
            "rewindtoken question",
            "2026-08-31T12:01:00.000Z",
        ))
        .expect("应能写入 user 消息");
    store
        .append_message(&NewMessage::new(
            session_id_type.clone(),
            "assistant",
            "rewindtoken answer",
            "2026-08-31T12:02:00.000Z",
        ))
        .expect("应能写入 assistant 消息");
    drop(store);
    let message_id_text = user_message_id.get().to_string();

    let rewound = parse_json(&run(
        &home,
        &[
            "--format",
            "json",
            "session",
            "rewind",
            session_id,
            &message_id_text,
        ],
    ));
    assert_eq!(rewound["operation"], "rewind");
    assert_eq!(rewound["target_message_id"], user_message_id.get());
    assert_eq!(rewound["rewound_count"], 2);
    assert!(rewound["new_head_id"].is_null());

    let detail = parse_json(&run(
        &home,
        &["--format", "json", "session", "show", session_id],
    ));
    assert!(
        detail["messages"]
            .as_array()
            .expect("消息应为数组")
            .is_empty()
    );
    assert_eq!(detail["session"]["message_count"], 0);

    let store = Store::open_readonly(&home.join("state.db")).expect("应能只读打开 state.db");
    let mut audit_query = sagent_store::MessageSearchQuery::new("rewindtoken");
    audit_query.include_inactive = true;
    assert_eq!(
        store
            .search_messages(&audit_query)
            .expect("审计搜索应读取回退消息")
            .len(),
        2
    );
    drop(store);
    fs::remove_dir_all(home).expect("应能清理测试目录");
}
