//! SessionActor 的 worker 工具调用与工具结果事件处理。
//!
//! 本模块只处理 Provider/tool worker 的事件边界：先验证当前 Turn 和调用协议，再由
//! Store 提交事实、安装 pending batch，最后交给现有工具审批/执行回环继续推进。

use sagent_agent::TurnState;
use sagent_store::NewMessage;
use sagent_types::TurnId;

use super::SessionActor;
use crate::{
    active_turn::PendingToolBatch,
    event::{RuntimeEvent, RuntimeEventKind},
    tool_call::ToolCall,
    tool_worker::ToolExecutionResult,
};

impl SessionActor {
    /// 校验并提交 Provider 的工具调用批次，然后启动 Actor 管理的工具回环。
    ///
    /// assistant/tool-call 事实必须先成功写入 Store，Actor 才能安装 pending batch 并
    /// 发布请求事件；任何校验、序列化或持久化失败都会进入统一 Turn 失败路径。
    pub(super) async fn apply_tool_calls(
        &mut self,
        turn_id: TurnId,
        text: String,
        calls: Vec<ToolCall>,
    ) {
        if !self.is_active_turn(turn_id) || self.is_cancelled(turn_id) {
            return;
        }
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.tool_rounds >= self.policy.max_tool_rounds)
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
            .tool_runtime
            .dispatcher()
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
        let mut assistant = NewMessage::new(
            self.context.session_id.clone(),
            "assistant",
            text,
            (self.context.clock)(),
        );
        assistant.tool_calls = Some(tool_calls);
        assistant.finish_reason = Some("tool_calls".into());
        if let Err(error) = self.context.session_storage.commit_assistant_tool_calls(
            &turn_id,
            &assistant,
            &(self.context.clock)(),
        ) {
            let _ = self
                .fail_active(turn_id, "persistence", error.to_string())
                .await;
            return;
        }
        if let Some(active) = self.active.as_mut() {
            active.state = TurnState::RunningTool;
            active.tool_rounds += 1;
            active.pending_tool_batch = Some(PendingToolBatch::new(plans.clone()));
        }
        for plan in &plans {
            self.publish(RuntimeEvent {
                session_id: self.context.session_id.clone(),
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

    /// 校验单个工具结果与 pending batch 的调用 ID，并在提交后推进下一个工具。
    pub(super) async fn apply_tool_results(
        &mut self,
        turn_id: TurnId,
        results: Vec<ToolExecutionResult>,
    ) {
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
