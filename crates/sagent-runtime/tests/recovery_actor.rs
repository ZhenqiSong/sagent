use std::{fs, path::PathBuf};

use sagent_runtime::SessionSupervisor;
use sagent_store::{
    EVENT_TOOL_STARTED, MessageQuery, NewDaemonEvent, NewGeneration, NewMessage, NewSession,
    StartTurn, Store,
};
use sagent_types::{SessionId, TurnId};

fn test_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "sagent-runtime-recovery-actor-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间有效")
            .as_nanos()
    ))
}

#[tokio::test]
async fn startup_records_unknown_tool_result_without_reexecuting_and_allows_a_new_turn() {
    let path = test_path();
    let session_id = SessionId::new("recovery-actor-session");
    let old_turn = TurnId::new();
    {
        let mut store = Store::open_readwrite(&path).expect("应能创建测试数据库");
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: Some("recovery-test".into()),
                model: Some("mock".into()),
                title: None,
                started_at: "00000000000000000001".into(),
            })
            .expect("应能创建会话");
        store
            .create_generation(&NewGeneration {
                session_id: session_id.clone(),
                generation: 0,
                system_hash: "system".into(),
                tool_schema_hash: "tools".into(),
                model_id: "mock".into(),
                profile_revision: "test".into(),
                created_at: "00000000000000000001".into(),
            })
            .expect("应能创建 generation");
        store
            .begin_turn(
                &StartTurn {
                    turn_id: old_turn,
                    session_id: session_id.clone(),
                    generation: 0,
                    started_at: "00000000000000000002".into(),
                },
                &NewMessage::new(session_id.clone(), "user", "旧请求", "00000000000000000002"),
            )
            .expect("应能创建旧 Turn");
        let mut assistant =
            NewMessage::new(session_id.clone(), "assistant", "", "00000000000000000003");
        assistant.tool_calls = Some(
            serde_json::json!([{
                "call_id": "call_unfinished",
                "name": "terminal",
                "arguments": {"command": "rm -rf __never_retry__"}
            }])
            .to_string(),
        );
        assistant.finish_reason = Some("tool_calls".into());
        store
            .commit_assistant_tool_calls(&old_turn, &assistant, "00000000000000000003")
            .expect("应能写入 assistant tool call");
        store
            .append_event(&NewDaemonEvent {
                session_id: session_id.clone(),
                turn_id: Some(old_turn),
                event_type: EVENT_TOOL_STARTED.into(),
                payload: serde_json::json!({
                    "provider_call_id": "call_unfinished",
                    "tool_name": "terminal",
                }),
                created_at: "00000000000000000004".into(),
            })
            .expect("应能写入 tool.started");
    }

    let factory_path = path.clone();
    let supervisor = SessionSupervisor::new(move || {
        Store::open_readwrite(&factory_path).map_err(|error| error.to_string())
    });
    let handle = supervisor
        .get_or_start(session_id.clone())
        .await
        .expect("应能启动恢复 actor");
    // Actor 在消费此 Close 命令前先完成旧 Turn 的恢复收口；恢复路径不会把旧工具
    // 当作可继续执行的任务，也不会启动任何 Provider 或 ToolWorker。
    handle.close().await.expect("应能关闭测试 actor");

    let store = Store::open_readonly(&path).expect("应能读取恢复后的数据库");
    let messages = store
        .get_messages_for_display(&session_id, &MessageQuery::default())
        .expect("应能读取消息");
    let recovered = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("call_unfinished"))
        .expect("未完成工具应得到恢复结果");
    assert_eq!(recovered.role, "tool");
    assert_eq!(
        recovered.content,
        "Runtime 重启时工具执行结果未知，未自动重试"
    );
    assert!(
        recovered
            .display_metadata
            .as_deref()
            .is_some_and(|metadata| metadata.contains("runtime_restarted_unknown"))
    );
    assert_eq!(
        store
            .get_running_turn(&session_id)
            .expect("应能查询 running Turn"),
        None,
        "旧 Turn 已由恢复逻辑收口"
    );

    let _ = fs::remove_file(path);
}
