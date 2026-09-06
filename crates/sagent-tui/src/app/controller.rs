//! 会话 picker 的副作用协调层。
//!
//! reducer 只做确定性状态转换；本模块是唯一允许把用户 action 转成 RPC 请求的位置。
//! 它始终将响应重新包装为 action，确保 UI 不因网络 I/O 直接修改 ViewModel。

use anyhow::Result;
use sagent_protocol::{
    ApprovalDecisionDto, ApprovalRespondParams, PromptSubmitParams, SessionCreateParams,
    SessionEventsSinceParams, SessionInterruptParams, SessionListParams, SessionResumeParams,
};
use sagent_types::{ApprovalId, EventSequence, SessionId, TurnId};
use serde_json::Value;

use super::{AppAction, AppState, Overlay, reduce};
use crate::rpc::RpcClient;

const SESSION_LIST_LIMIT: u32 = 100;
const RESUME_MESSAGE_LIMIT: u32 = 200;

/// 加载当前 Profile 的非归档会话，并在成功后打开可操作的 picker。
pub async fn load_sessions(client: &RpcClient, state: &mut AppState) -> Result<()> {
    reduce(state, AppAction::OpenSessionPicker);
    let result = client
        .list_sessions(SessionListParams {
            include_archived: false,
            limit: Some(SESSION_LIST_LIMIT),
            offset: 0,
        })
        .await;
    match result {
        Ok(result) => reduce(state, AppAction::SessionsLoaded(result)),
        Err(error) => reduce(
            state,
            AppAction::SessionOperationFailed {
                message: format!("加载会话失败：{error:#}"),
            },
        ),
    }
    Ok(())
}

/// 重连后用服务端快照恢复此前活动会话；没有活动会话时退回正常列表加载。
pub async fn restore_active_session(client: &RpcClient, state: &mut AppState) -> Result<()> {
    let Some(session_id) = state
        .active_session
        .as_ref()
        .map(|session| session.summary.id.clone())
    else {
        return load_sessions(client, state).await;
    };
    // 断线前的 delta 已被 reducer 清除；resume 是唯一能重建 transcript 的事实来源。
    resume_session(client, state, session_id.clone()).await?;
    replay_persisted_events(client, state, session_id).await
}

/// 按 sequence 分页补读持久化事件。消息内容已经由 resume 恢复；这里主要恢复 Turn、
/// 工具和审批终态的检查点，绝不尝试重放瞬态 `message.delta`。
async fn replay_persisted_events(
    client: &RpcClient,
    state: &mut AppState,
    session_id: String,
) -> Result<()> {
    let mut after = state
        .checkpoint
        .as_ref()
        .filter(|checkpoint| checkpoint.session_id == session_id)
        .map_or(0, |checkpoint| checkpoint.last_sequence);
    loop {
        let after_i64 =
            i64::try_from(after).map_err(|_| anyhow::anyhow!("事件检查点超出协议范围"))?;
        let result = client
            .events_since(SessionEventsSinceParams {
                session_id: SessionId::new(session_id.clone()),
                after_sequence: EventSequence::new(after_i64)
                    .map_err(|error| anyhow::anyhow!("事件检查点无效：{error}"))?,
                limit: Some(200),
            })
            .await?;
        for event in result.events {
            let sequence = u64::try_from(event.sequence.get())
                .map_err(|_| anyhow::anyhow!("服务端返回负事件序号"))?;
            // 服务端可能重复返回边界 event；只推进严格大于本地检查点的事实。
            if sequence > after && event.session_id.as_str() == session_id {
                after = sequence;
            }
        }
        let latest = u64::try_from(result.latest_sequence.get())
            .map_err(|_| anyhow::anyhow!("服务端返回负最新事件序号"))?;
        after = after.max(latest);
        state.checkpoint = Some(super::state::SessionCheckpoint {
            session_id: session_id.clone(),
            last_sequence: after,
        });
        if !result.has_more {
            return Ok(());
        }
    }
}

/// 处理当前步骤的会话 action；不属于会话的 action 仅交给 reducer。
pub async fn handle_action(
    client: &RpcClient,
    state: &mut AppState,
    action: AppAction,
) -> Result<()> {
    match action {
        AppAction::ConfirmSessionSelection => resume_selected_session(client, state).await,
        AppAction::CreateSessionRequested => create_then_resume(client, state).await,
        AppAction::SubmitPromptRequested => submit_prompt(client, state).await,
        AppAction::InterruptRequested => interrupt_turn(client, state).await,
        AppAction::ApprovalDecisionRequested(decision) => {
            respond_approval(client, state, decision).await
        }
        other => {
            reduce(state, other);
            Ok(())
        }
    }
}

/// 同一 Turn 的 interrupt 只允许送入 Runtime mailbox 一次；响应不是终态。
async fn interrupt_turn(client: &RpcClient, state: &mut AppState) -> Result<()> {
    let Some(active) = state.active_turn.as_ref() else {
        return Ok(());
    };
    if active.interrupt_pending {
        return Ok(());
    }
    match client
        .interrupt_session(SessionInterruptParams {
            session_id: SessionId::new(active.session_id.clone()),
        })
        .await
    {
        Ok(_) => reduce(state, AppAction::InterruptAccepted),
        Err(error) => reduce(
            state,
            AppAction::SessionOperationFailed {
                message: format!("中断请求失败：{error:#}"),
            },
        ),
    }
    Ok(())
}

/// 提交审批决定；失败时保留弹层，绝不自动替用户拒绝工具。
async fn respond_approval(
    client: &RpcClient,
    state: &mut AppState,
    decision: ApprovalDecisionDto,
) -> Result<()> {
    let Overlay::Approval(approval) = &state.overlay else {
        return Ok(());
    };
    if approval.submitting {
        return Ok(());
    }
    let params = ApprovalRespondParams {
        session_id: SessionId::new(approval.session_id.clone()),
        turn_id: TurnId::parse(&approval.turn_id)
            .map_err(|error| anyhow::anyhow!("审批 turn id 无效：{error}"))?,
        approval_id: ApprovalId::parse(&approval.approval_id)
            .map_err(|error| anyhow::anyhow!("审批 id 无效：{error}"))?,
        decision,
    };
    reduce(state, AppAction::ApprovalDecisionRequested(decision));
    match client.respond_approval(params).await {
        Ok(_) => reduce(state, AppAction::ApprovalResponseAccepted),
        Err(error) => {
            if let Overlay::Approval(approval) = &mut state.overlay {
                approval.submitting = false;
                approval.error = Some(format!("提交审批失败：{error:#}"));
            }
        }
    }
    Ok(())
}

/// 将 JSON-RPC event 解析为最小 UI 事实；TUI 不依赖 Runtime crate 或内部 Actor 类型。
pub async fn handle_rpc_event(
    client: &RpcClient,
    state: &mut AppState,
    event: sagent_protocol::JsonRpcEvent<Value>,
) -> Result<()> {
    let payload = event.params.payload;
    let Some(session_id) = payload.get("session_id").and_then(Value::as_str) else {
        return Ok(());
    };
    let turn_id = payload
        .get("turn_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match event.params.event_type.as_str() {
        "message.delta" => {
            if let Some(text) = payload
                .get("data")
                .and_then(|data| data.get("text"))
                .and_then(Value::as_str)
            {
                reduce(
                    state,
                    AppAction::StreamDeltaReceived {
                        session_id: session_id.to_owned(),
                        turn_id: turn_id.to_owned(),
                        text: text.to_owned(),
                    },
                );
            }
        }
        "message.complete" | "turn.completed" | "turn.interrupted" | "turn.failed" => {
            let matches_active = state
                .active_turn
                .as_ref()
                .is_some_and(|active| active.session_id == session_id && active.turn_id == turn_id);
            if matches_active {
                // Turn 已终态时原审批不再有效，先关闭弹层再用持久化快照刷新 transcript。
                if matches!(&state.overlay, Overlay::Approval(approval) if approval.session_id == session_id && approval.turn_id == turn_id)
                {
                    reduce(state, AppAction::DismissOverlay);
                }
                reduce(
                    state,
                    AppAction::ActiveTurnFinished {
                        session_id: session_id.to_owned(),
                        turn_id: turn_id.to_owned(),
                    },
                );
                resume_session(client, state, session_id.to_owned()).await?;
            }
        }
        "approval.requested" => {
            let data = payload.get("data");
            if let (Some(approval_id), Some(tool_name), Some(summary), Some(expires_at)) = (
                data.and_then(|value| value.get("approval_id"))
                    .and_then(Value::as_str),
                data.and_then(|value| value.get("tool_name"))
                    .and_then(Value::as_str),
                data.and_then(|value| value.get("summary"))
                    .and_then(Value::as_str),
                data.and_then(|value| value.get("expires_at"))
                    .and_then(Value::as_str),
            ) {
                reduce(
                    state,
                    AppAction::ApprovalRequested {
                        approval_id: approval_id.to_owned(),
                        session_id: session_id.to_owned(),
                        turn_id: turn_id.to_owned(),
                        tool_name: tool_name.to_owned(),
                        summary: summary.to_owned(),
                        expires_at: expires_at.to_owned(),
                    },
                );
            }
        }
        "approval.timed_out" | "approval.resolved" => {
            if matches!(&state.overlay, Overlay::Approval(approval) if approval.session_id == session_id && approval.turn_id == turn_id)
            {
                reduce(state, AppAction::DismissOverlay);
            }
        }
        "tool.requested" | "tool.started" | "tool.completed" => {
            let data = payload.get("data");
            if let (Some(call_id), Some(tool_name)) = (
                data.and_then(|value| value.get("call_id"))
                    .and_then(Value::as_str),
                data.and_then(|value| value.get("tool_name"))
                    .and_then(Value::as_str),
            ) {
                let running = match event.params.event_type.as_str() {
                    "tool.started" => Some(true),
                    "tool.completed" => Some(false),
                    _ => None,
                };
                reduce(
                    state,
                    AppAction::ToolActivityReceived {
                        call_id: call_id.to_owned(),
                        tool_name: tool_name.to_owned(),
                        running,
                    },
                );
            }
        }
        _ => {}
    }
    Ok(())
}

/// 校验 composer 后提交；失败时不改变 composer，避免丢失用户尚未发送的文本。
async fn submit_prompt(client: &RpcClient, state: &mut AppState) -> Result<()> {
    let Some(session) = state.active_session.as_ref() else {
        reduce(
            state,
            AppAction::SessionOperationFailed {
                message: "请先选择会话".to_owned(),
            },
        );
        return Ok(());
    };
    if state.active_turn.is_some() || state.composer.text.trim().is_empty() {
        return Ok(());
    }
    let params = PromptSubmitParams {
        session_id: SessionId::new(session.summary.id.clone()),
        text: state.composer.text.clone(),
    };
    match client.submit_prompt(params).await {
        Ok(result) => reduce(state, AppAction::PromptSubmitted(result)),
        Err(error) => reduce(
            state,
            AppAction::SessionOperationFailed {
                message: format!("提交消息失败：{error:#}"),
            },
        ),
    }
    Ok(())
}

/// 使用 picker 当前选中的服务端 id 恢复历史；空列表时不发送无效请求。
async fn resume_selected_session(client: &RpcClient, state: &mut AppState) -> Result<()> {
    let Some(session_id) = selected_session_id(state) else {
        return Ok(());
    };
    resume_session(client, state, session_id).await
}

/// 先创建，再使用服务端实际返回的 id 恢复；绝不由 TUI 生成或猜测会话标识。
async fn create_then_resume(client: &RpcClient, state: &mut AppState) -> Result<()> {
    set_picker_loading(state, true);
    let created = client.create_session(SessionCreateParams::default()).await;
    let created = match created {
        Ok(created) => created,
        Err(error) => {
            reduce(
                state,
                AppAction::SessionOperationFailed {
                    message: format!("创建会话失败：{error:#}"),
                },
            );
            return Ok(());
        }
    };
    resume_session(client, state, created.session_id.as_str().to_owned()).await
}

/// 以 `session.resume` 快照替换当前 transcript；失败时保留旧视图和 picker。
async fn resume_session(
    client: &RpcClient,
    state: &mut AppState,
    session_id: String,
) -> Result<()> {
    set_picker_loading(state, true);
    let result = client
        .resume_session(SessionResumeParams {
            session_id,
            message_limit: Some(RESUME_MESSAGE_LIMIT),
            message_offset: 0,
        })
        .await;
    match result {
        Ok(result) => reduce(state, AppAction::SessionResumed(Box::new(result))),
        Err(error) => reduce(
            state,
            AppAction::SessionOperationFailed {
                message: format!("恢复会话失败：{error:#}"),
            },
        ),
    }
    Ok(())
}

/// 取得当前选项的 id；索引失效时安全地视为无选择，不向 RPC 发请求。
fn selected_session_id(state: &AppState) -> Option<String> {
    let Overlay::SessionPicker {
        selected_index,
        loading,
    } = state.overlay
    else {
        return None;
    };
    if loading {
        return None;
    }
    state
        .sessions
        .get(selected_index)
        .map(|session| session.id.clone())
}

/// 请求执行期间锁定 picker，防止重复 Enter/n 产生并发 resume 或重复 create。
fn set_picker_loading(state: &mut AppState, loading: bool) {
    if let Overlay::SessionPicker {
        loading: current, ..
    } = &mut state.overlay
    {
        *current = loading;
    }
}

#[cfg(test)]
mod tests {
    use sagent_protocol::SessionSummaryDto;

    use super::selected_session_id;
    use crate::app::{AppState, Overlay};

    #[test]
    fn selected_session_id_never_guesses_an_out_of_range_or_loading_selection() {
        let mut state = AppState {
            sessions: vec![SessionSummaryDto {
                id: "server-session".to_owned(),
                source: None,
                model: None,
                title: None,
                started_at: None,
                ended_at: None,
                end_reason: None,
                last_active: None,
                preview: None,
                message_count: 0,
            }],
            overlay: Overlay::SessionPicker {
                selected_index: 2,
                loading: false,
            },
            ..AppState::default()
        };
        assert_eq!(selected_session_id(&state), None);

        state.overlay = Overlay::SessionPicker {
            selected_index: 0,
            loading: true,
        };
        assert_eq!(selected_session_id(&state), None);
    }
}
