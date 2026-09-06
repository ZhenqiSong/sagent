//! Runtime 重启后的只读恢复判定。
//!
//! 判定器只读取 Store 中已经提交的事实，绝不启动 Provider、ToolWorker 或子进程。
//! 对于已开始但没有结果的工具调用，无法可靠断言副作用是否已经发生，因此统一
//! 选择 fail-closed：写入“结果未知”而不是重新执行。

use std::collections::{HashMap, HashSet};

use sagent_store::{
    EVENT_APPROVAL_REQUESTED, EVENT_APPROVAL_RESOLVED, EVENT_APPROVAL_TIMED_OUT,
    EVENT_MESSAGE_COMMITTED, EVENT_TOOL_COMPLETED, EVENT_TOOL_STARTED, MAX_EVENT_LIMIT, Store,
};
use sagent_types::{EventSequence, SessionId, TurnId};

use crate::ToolCall;

/// 一个尚未得到持久化结果的工具调用，以及重启后应写入的确定性错误。
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct RecoveredToolCall {
    pub(crate) call_id: String,
    pub(crate) name: String,
    pub(crate) error_kind: &'static str,
    pub(crate) message: &'static str,
}

/// 一个遗留 running Turn 的安全收口计划。
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct RecoveryPlan {
    pub(crate) turn_id: TurnId,
    pub(crate) unresolved_tools: Vec<RecoveredToolCall>,
}

/// 从一个 Session 的事实事件构造恢复计划。没有 running Turn 时返回 None。
pub(crate) fn plan_recovery(
    store: &Store,
    session_id: &SessionId,
) -> Result<Option<RecoveryPlan>, String> {
    let Some(turn) = store
        .get_running_turn(session_id)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let events = turn_events(store, turn.turn_id)?;
    let mut calls = Vec::new();
    let mut completed = HashSet::new();
    let mut started = HashSet::new();
    let mut approval_calls = HashMap::new();
    let mut pending_approvals = HashSet::new();

    for event in events {
        match event.event_type.as_str() {
            EVENT_MESSAGE_COMMITTED
                if event
                    .payload
                    .get("role")
                    .and_then(serde_json::Value::as_str)
                    == Some("assistant") =>
            {
                if let Some(tool_calls) = event.payload.get("tool_calls") {
                    let parsed = serde_json::from_value::<Vec<ToolCall>>(tool_calls.clone())
                        .map_err(|error| {
                            format!("解析恢复中的 assistant tool_calls 失败：{error}")
                        })?;
                    calls.extend(parsed);
                }
            }
            EVENT_TOOL_STARTED => {
                if let Some(call_id) = event
                    .payload
                    .get("provider_call_id")
                    .and_then(serde_json::Value::as_str)
                {
                    started.insert(call_id.to_owned());
                }
            }
            EVENT_TOOL_COMPLETED => {
                if let Some(call_id) = event
                    .payload
                    .get("tool_call_id")
                    .and_then(serde_json::Value::as_str)
                {
                    completed.insert(call_id.to_owned());
                }
            }
            EVENT_APPROVAL_REQUESTED => {
                let Some(approval_id) = event
                    .payload
                    .get("approval_id")
                    .and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                let Some(call_id) = event
                    .payload
                    .get("provider_call_id")
                    .and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                approval_calls.insert(approval_id.to_owned(), call_id.to_owned());
                pending_approvals.insert(call_id.to_owned());
            }
            EVENT_APPROVAL_RESOLVED | EVENT_APPROVAL_TIMED_OUT => {
                if let Some(approval_id) = event
                    .payload
                    .get("approval_id")
                    .and_then(serde_json::Value::as_str)
                    && let Some(call_id) = approval_calls.get(approval_id)
                {
                    pending_approvals.remove(call_id);
                }
            }
            _ => {}
        }
    }

    let unresolved_tools = calls
        .into_iter()
        .filter(|call| !completed.contains(&call.call_id))
        .map(|call| {
            let (error_kind, message) = if started.contains(&call.call_id) {
                (
                    "runtime_restarted_unknown",
                    "Runtime 重启时工具执行结果未知，未自动重试",
                )
            } else if pending_approvals.contains(&call.call_id) {
                (
                    "approval_interrupted_by_restart",
                    "Runtime 重启时审批未完成，工具未执行",
                )
            } else {
                (
                    "runtime_restarted_not_executed",
                    "Runtime 重启前工具尚未执行，未自动重试",
                )
            };
            RecoveredToolCall {
                call_id: call.call_id,
                name: call.name,
                error_kind,
                message,
            }
        })
        .collect();
    Ok(Some(RecoveryPlan {
        turn_id: turn.turn_id,
        unresolved_tools,
    }))
}

fn turn_events(
    store: &Store,
    turn_id: TurnId,
) -> Result<Vec<sagent_store::StoredDaemonEvent>, String> {
    let mut after = EventSequence::default();
    let mut events = Vec::new();
    loop {
        let page = store
            .events_for_turn(&turn_id, after)
            .map_err(|error| error.to_string())?;
        let page_len = page.len();
        if let Some(last) = page.last() {
            after = last.sequence;
        }
        events.extend(page);
        if page_len < MAX_EVENT_LIMIT as usize {
            return Ok(events);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::plan_recovery;
    use sagent_store::{NewDaemonEvent, NewGeneration, NewMessage, NewSession, StartTurn, Store};
    use sagent_types::{SessionId, TurnId};

    fn store_with_turn() -> (Store, std::path::PathBuf, SessionId, TurnId) {
        let path = std::env::temp_dir().join(format!(
            "sagent-runtime-recovery-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("系统时间有效")
                .as_nanos()
        ));
        let session_id = SessionId::new("recovery-session");
        let turn_id = TurnId::new();
        let mut store = Store::open_readwrite(&path).expect("应能打开 Store");
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: None,
                model: None,
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
                    turn_id,
                    session_id: session_id.clone(),
                    generation: 0,
                    started_at: "00000000000000000002".into(),
                },
                &NewMessage::new(session_id.clone(), "user", "继续", "00000000000000000002"),
            )
            .expect("应能开始 Turn");
        (store, path, session_id, turn_id)
    }

    #[test]
    fn pending_approval_is_classified_as_not_executed_after_restart() {
        let (mut store, path, session_id, turn_id) = store_with_turn();
        let mut assistant =
            NewMessage::new(session_id.clone(), "assistant", "", "00000000000000000003");
        assistant.tool_calls = Some(
            serde_json::json!([{"call_id":"call-terminal","name":"terminal","arguments":{"command":"rm -rf x"}}])
                .to_string(),
        );
        assistant.finish_reason = Some("tool_calls".into());
        store
            .commit_assistant_tool_calls(&turn_id, &assistant, "00000000000000000003")
            .expect("应能写入 tool calls");
        store
            .append_event(&NewDaemonEvent {
                session_id: session_id.clone(),
                turn_id: Some(turn_id),
                event_type: "approval.requested".into(),
                payload: serde_json::json!({
                    "approval_id": sagent_types::ApprovalId::new(),
                    "provider_call_id": "call-terminal",
                }),
                created_at: "00000000000000000004".into(),
            })
            .expect("应能写入审批事实");

        let plan = plan_recovery(&store, &session_id)
            .expect("恢复计划应可读取")
            .expect("应存在 running Turn");
        assert_eq!(plan.unresolved_tools.len(), 1);
        assert_eq!(
            plan.unresolved_tools[0].error_kind,
            "approval_interrupted_by_restart"
        );
        drop(store);
        let _ = std::fs::remove_file(path);
    }
}
