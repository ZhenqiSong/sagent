//! SessionActor 的 Provider/Tool worker 事件归并。
//!
//! worker 只投递事实，所有状态转换、Store 写入和事件发布仍在 Actor mailbox 中串行完成，
//! 从而阻止迟到结果与终态收口竞争。

use sagent_agent::TurnState;
use sagent_store::NewMessage;
use sagent_types::TurnId;

use super::SessionActor;
use crate::{
    active_turn::PendingToolBatch,
    event::{RuntimeEvent, RuntimeEventKind},
    input::WorkerEvent,
};

impl SessionActor {
    pub(super) async fn handle_worker_event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::TextDelta { turn_id, text } => {
                if self.is_active_turn(turn_id) && !self.is_cancelled(turn_id) {
                    self.publish(RuntimeEvent {
                        session_id: self.session_id.clone(),
                        turn_id: Some(turn_id),
                        request_id: self.active.as_ref().map(|turn| turn.request_id),
                        kind: RuntimeEventKind::ModelTextDelta { text },
                    });
                }
            }
            WorkerEvent::Usage { turn_id, usage } => {
                if self.is_active_turn(turn_id) && !self.is_cancelled(turn_id) {
                    self.publish(RuntimeEvent {
                        session_id: self.session_id.clone(),
                        turn_id: Some(turn_id),
                        request_id: self.active.as_ref().map(|turn| turn.request_id),
                        kind: RuntimeEventKind::ModelUsage { usage },
                    });
                }
            }
            WorkerEvent::FinalText { turn_id, text } => {
                let _ = self.complete_active(turn_id, text).await;
            }
            WorkerEvent::Failed { turn_id, reason } => {
                let _ = self.fail_active(turn_id, "worker", reason).await;
            }
            WorkerEvent::Cancelled { turn_id } => {
                let _ = self
                    .interrupt_active_for_turn(turn_id, "worker cancelled")
                    .await;
            }
            WorkerEvent::ToolCalls {
                turn_id,
                text,
                calls,
            } => {
                if !self.is_active_turn(turn_id) || self.is_cancelled(turn_id) {
                    return;
                }
                if self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.tool_rounds >= self.max_tool_rounds)
                {
                    let _ = self
                        .fail_active(
                            turn_id,
                            "tool_loop_limit",
                            "工具调用超过最大回环次数".to_owned(),
                        )
                        .await;
                    return;
                }
                if calls.is_empty() {
                    let _ = self
                        .fail_active(
                            turn_id,
                            "tool_dispatch",
                            "Provider 返回了空工具调用列表".to_owned(),
                        )
                        .await;
                    return;
                }
                let plans = match self
                    .tool_dispatcher
                    .as_ref()
                    .ok_or_else(|| "ToolRegistry 未配置".to_owned())
                    .and_then(|dispatcher| {
                        dispatcher
                            .plan(calls.clone())
                            .map_err(|error| error.to_string())
                    }) {
                    Ok(plans) => plans,
                    Err(reason) => {
                        let _ = self.fail_active(turn_id, "tool_dispatch", reason).await;
                        return;
                    }
                };
                let tool_calls = match serde_json::to_string(&calls) {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = self
                            .fail_active(turn_id, "tool_dispatch", error.to_string())
                            .await;
                        return;
                    }
                };
                let mut assistant =
                    NewMessage::new(self.session_id.clone(), "assistant", text, (self.clock)());
                assistant.tool_calls = Some(tool_calls);
                assistant.finish_reason = Some("tool_calls".into());
                if let Err(error) =
                    self.store
                        .commit_assistant_tool_calls(&turn_id, &assistant, &(self.clock)())
                {
                    let _ = self
                        .fail_active(turn_id, "persistence", error.to_string())
                        .await;
                    return;
                }
                if let Some(active) = self.active.as_mut() {
                    active.state = TurnState::RunningTool;
                    active.tool_rounds += 1;
                    active.provider_exit_credits += 1;
                    active.pending_tool_batch = Some(PendingToolBatch::new(plans.clone()));
                }
                for plan in &plans {
                    self.publish(RuntimeEvent {
                        session_id: self.session_id.clone(),
                        turn_id: Some(turn_id),
                        request_id: self.active.as_ref().map(|active| active.request_id),
                        kind: RuntimeEventKind::ToolCallRequested {
                            call_id: plan.call.call_id.clone(),
                            tool_name: plan.call.name.clone(),
                        },
                    });
                }
                self.advance_tool_batch(turn_id).await;
            }
            WorkerEvent::ToolResults { turn_id, results } => {
                if !self.is_active_turn(turn_id) || self.is_cancelled(turn_id) {
                    return;
                }
                let [result] = results.as_slice() else {
                    let _ = self
                        .fail_active(
                            turn_id,
                            "tool_protocol",
                            "工具 worker 必须一次返回当前调用的一个结果".into(),
                        )
                        .await;
                    return;
                };
                let expected_call_id = self
                    .active
                    .as_ref()
                    .and_then(|active| active.pending_tool_batch.as_ref())
                    .and_then(PendingToolBatch::current)
                    .map(|plan| plan.call.call_id.clone());
                if expected_call_id.as_deref() != Some(result.call_id.as_str()) {
                    let _ = self
                        .fail_active(
                            turn_id,
                            "tool_protocol",
                            "工具结果与当前等待的调用不匹配".into(),
                        )
                        .await;
                    return;
                }
                if let Err(error) = self.persist_tool_result(turn_id, result.clone()) {
                    let _ = self
                        .fail_active(turn_id, "persistence", error.to_string())
                        .await;
                    return;
                }
                if let Some(active) = self.active.as_mut()
                    && let Some(batch) = active.pending_tool_batch.as_mut()
                {
                    // 当前 task 已经成功把结果投递到 mailbox；不再由终态路径等待它，
                    // 避免下一项工具错误地继承旧 JoinHandle。
                    active.tool_task.take();
                    batch.complete_current();
                }
                self.advance_tool_batch(turn_id).await;
            }
        }
    }

    pub(super) async fn handle_worker_exited(
        &mut self,
        turn_id: TurnId,
        result: Result<(), crate::input::WorkerFailure>,
    ) {
        if !self.is_active_turn(turn_id) || self.is_cancelled(turn_id) {
            return;
        }
        if result.is_ok()
            && let Some(active) = self.active.as_mut()
            && active.provider_exit_credits > 0
        {
            // Provider 已经把控制权交给 ToolWorker；该退出即使晚于下一轮
            // Provider 启动，也只能消费自己的 credit，不能终止新一轮。
            active.provider_exit_credits -= 1;
            return;
        }
        if let Err(error) = result {
            let _ = self.fail_active(turn_id, "worker", error.0).await;
        } else {
            let _ = self
                .fail_active(turn_id, "worker_exit", "worker 未产生最终结果".into())
                .await;
        }
    }
}
