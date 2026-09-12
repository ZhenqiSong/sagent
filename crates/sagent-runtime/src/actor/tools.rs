//! SessionActor 的工具调度与审批回环。
//!
//! 该子模块保持为 `actor` 的私有子模块：工具、审批和持久化推进必须通过同一个
//! mailbox 所属 Actor 串行执行，不能被 worker 或 RPC transport 直接调用。

use sagent_agent::{ApprovalDecision, TurnState};
use sagent_store::{
    EVENT_APPROVAL_REQUESTED, EVENT_APPROVAL_RESOLVED, EVENT_APPROVAL_TIMED_OUT,
    EVENT_TOOL_STARTED, NewDaemonEvent, NewMessage,
};
use sagent_tools::{ApprovalPolicyDecision, classify_tool};
use sagent_types::{ToolCallId, TurnId};

use super::SessionActor;
use crate::{
    RuntimeError,
    active_turn::PendingToolBatch,
    approval::{ApprovalOutcome, ApprovalRequest},
    event::{RuntimeEvent, RuntimeEventKind},
    input::{ActorInput, CommandReply, WorkerEvent},
    tool_worker::ToolExecutionResult,
};

impl SessionActor {
    /// 推进当前 Provider 产生的工具批次；每次只启动一个工具，因而能在危险调用前
    /// 安全暂停并在审批后从同一位置恢复。
    pub(super) async fn advance_tool_batch(&mut self, turn_id: TurnId) {
        if !self.is_active_turn(turn_id) || self.is_cancelled(turn_id) {
            return;
        }
        let (plan, approved) = match self
            .active
            .as_ref()
            .and_then(|active| active.pending_tool_batch.as_ref())
        {
            Some(batch) => match batch.current() {
                Some(plan) => (plan.clone(), batch.current_approved),
                None => {
                    if let Some(active) = self.active.as_mut() {
                        active.pending_tool_batch = None;
                    }
                    if let Err(error) = self.start_next_provider_round(turn_id) {
                        let _ = self
                            .fail_active(turn_id, "tool_loop", error.to_string())
                            .await;
                    }
                    return;
                }
            },
            None => return,
        };

        if approved {
            self.start_tool_plan(turn_id, plan).await;
            return;
        }
        match classify_tool(&plan.call.name, &plan.call.arguments) {
            ApprovalPolicyDecision::Allow => self.start_tool_plan(turn_id, plan).await,
            ApprovalPolicyDecision::RequireApproval {
                policy_key,
                summary,
            } => {
                if self.approvals.is_allowed(&self.session_id, &policy_key) {
                    self.start_tool_plan(turn_id, plan).await;
                    return;
                }
                let request = match ApprovalRequest::new(
                    self.session_id.clone(),
                    turn_id,
                    ToolCallId::new(),
                    plan.call.name,
                    summary,
                    policy_key,
                    self.approval_expires_at(),
                ) {
                    Ok(request) => request,
                    Err(error) => {
                        let _ = self
                            .fail_active(turn_id, "approval", error.to_string())
                            .await;
                        return;
                    }
                };
                self.handle_approval_required(turn_id, request).await;
            }
            ApprovalPolicyDecision::Deny { reason } => {
                self.fail_current_tool(turn_id, "tool_policy", "policy_denied", reason)
                    .await;
            }
        }
    }

    /// 启动已经通过动态风险策略的单个工具。工具 worker 不拥有审批状态，不能自行
    /// 决定执行危险命令。
    async fn start_tool_plan(&mut self, turn_id: TurnId, plan: crate::ToolDispatchPlan) {
        let Some(tool_worker) = self.tool_worker.clone() else {
            let _ = self
                .fail_active(turn_id, "tool_dispatch", "ToolWorker 未配置".to_owned())
                .await;
            return;
        };
        let cancellation = match self
            .active
            .as_ref()
            .filter(|active| active.turn_id == turn_id)
        {
            Some(active) => active.cancellation.child_token(),
            None => return,
        };
        if let Some(active) = self.active.as_mut() {
            active.state = TurnState::RunningTool;
        }
        if let Err(error) = self.persist_tool_started(turn_id, &plan) {
            let _ = self
                .fail_active(turn_id, "persistence", error.to_string())
                .await;
            return;
        }
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: self.active.as_ref().map(|active| active.request_id),
            kind: RuntimeEventKind::ToolStarted {
                call_id: plan.call.call_id.clone(),
                tool_name: plan.call.name.clone(),
            },
        });
        let sender = self.command_tx.clone();
        let tool_task = tokio::spawn(async move {
            let result = tool_worker.execute(plan, cancellation).await;
            let _ = sender
                .send(ActorInput::Worker(WorkerEvent::ToolResults {
                    turn_id,
                    results: vec![result],
                }))
                .await;
        });
        if let Some(active) = self.active.as_mut() {
            active.tool_task = Some(tool_task);
        }
    }

    /// 在启动真实 ToolWorker 前写入可恢复事实。恢复路径据此知道结果缺失时不能
    /// 重新运行同一调用，因为副作用可能已经发生。
    fn persist_tool_started(
        &mut self,
        turn_id: TurnId,
        plan: &crate::ToolDispatchPlan,
    ) -> Result<(), RuntimeError> {
        let mut payload = serde_json::json!({
            "provider_call_id": plan.call.call_id,
            "tool_name": plan.call.name,
        });
        if plan.call.name == "write_file" {
            // 审计需要能回答“哪个文件、是否请求覆盖、写入多少字节”，但 file content
            // 属于模型输入/用户数据，绝不能复制到 daemon event 或恢复日志。
            payload["write_file"] = serde_json::json!({
                "path": plan.call.arguments.get("path").and_then(serde_json::Value::as_str),
                "overwrite": plan.call.arguments.get("overwrite").and_then(serde_json::Value::as_bool).unwrap_or(false),
                "content_bytes": plan.call.arguments.get("content").and_then(serde_json::Value::as_str).map(str::len),
            });
        }
        self.store
            .append_event(&NewDaemonEvent {
                session_id: self.session_id.clone(),
                turn_id: Some(turn_id),
                event_type: EVENT_TOOL_STARTED.to_owned(),
                payload,
                created_at: (self.clock)(),
            })
            .map(|_| ())
            .map_err(|error| RuntimeError::Persistence(error.to_string()))
    }

    /// 将一个工具结果和其事件作为同一 Actor 操作提交；调用方随后才能推进游标。
    pub(super) fn persist_tool_result(
        &mut self,
        turn_id: TurnId,
        result: ToolExecutionResult,
    ) -> Result<(), RuntimeError> {
        let mut message = NewMessage::new(
            self.session_id.clone(),
            "tool",
            result.content.clone(),
            (self.clock)(),
        );
        message.tool_call_id = Some(result.call_id.clone());
        message.tool_name = Some(result.name.clone());
        message.finish_reason = Some("tool_completed".into());
        message.display_metadata = Some(
            serde_json::json!({
                "ok": result.ok,
                "truncated": result.truncated,
                "exit_code": result.exit_code,
                "error_kind": result.error_kind,
            })
            .to_string(),
        );
        self.store
            .commit_tool_result(&turn_id, &message, &(self.clock)())
            .map_err(|error| RuntimeError::Persistence(error.to_string()))?;
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: self.active.as_ref().map(|active| active.request_id),
            kind: RuntimeEventKind::ToolCompleted {
                call_id: result.call_id,
                tool_name: result.name,
                ok: result.ok,
                error_kind: result.error_kind,
            },
        });
        Ok(())
    }

    /// 对未启动的工具写入可恢复的失败结果，再收口当前 Turn。
    async fn fail_current_tool(
        &mut self,
        turn_id: TurnId,
        category: &str,
        error_kind: &str,
        reason: String,
    ) {
        let plan = self
            .active
            .as_ref()
            .and_then(|active| active.pending_tool_batch.as_ref())
            .and_then(PendingToolBatch::current)
            .cloned();
        let Some(plan) = plan else {
            let _ = self.fail_active(turn_id, category, reason).await;
            return;
        };
        let result = ToolExecutionResult {
            call_id: plan.call.call_id,
            name: plan.call.name,
            ok: false,
            content: reason.clone(),
            truncated: false,
            exit_code: None,
            error_kind: Some(error_kind.to_owned()),
        };
        if let Err(error) = self.persist_tool_result(turn_id, result) {
            let _ = self
                .fail_active(turn_id, "persistence", error.to_string())
                .await;
            return;
        }
        let _ = self.fail_active(turn_id, category, reason).await;
    }

    /// 生成展示用过期时间；实际 timeout 仍以 ApprovalManager 的单调时钟为准。
    fn approval_expires_at(&self) -> String {
        let now = (self.clock)();
        now.parse::<u64>()
            .map(|seconds| {
                format!(
                    "{:020}",
                    seconds.saturating_add(self.approvals.timeout().as_secs())
                )
            })
            .unwrap_or(now)
    }

    /// 接收工具 worker 的审批请求。这里不能直接 await waiter，否则 Actor 将无法处理
    /// `ResolveApproval` 或 `Interrupt`；waiter 只负责等待，结果通过 mailbox 回传。
    pub(super) async fn handle_approval_required(
        &mut self,
        turn_id: TurnId,
        request: ApprovalRequest,
    ) {
        if !self.is_active_turn(turn_id) || self.is_cancelled(turn_id) {
            return;
        }
        if request.session_id != self.session_id || request.turn_id != turn_id {
            let _ = self
                .fail_active(turn_id, "approval", "审批请求上下文不匹配".into())
                .await;
            return;
        }
        if !self.interactive_approval {
            self.fail_current_tool(
                turn_id,
                "approval_unavailable",
                "approval_unavailable",
                "当前客户端不支持交互式审批".into(),
            )
            .await;
            return;
        }
        if self
            .approvals
            .is_allowed(&self.session_id, &request.policy_key)
        {
            // 正常工具批次会在 advance_tool_batch 中先检查此规则并直接执行，
            // 因而这里仅保留外部注入 ApprovalRequired 的幂等兼容路径。
            return;
        }

        let approval_id = request.approval_id;
        let provider_call_id = self
            .active
            .as_ref()
            .and_then(|active| active.pending_tool_batch.as_ref())
            .and_then(PendingToolBatch::current)
            .map(|plan| plan.call.call_id.clone());
        let waiter = match self.approvals.register(request.clone()) {
            Ok(waiter) => waiter,
            Err(error) => {
                let _ = self
                    .fail_active(turn_id, "approval", error.to_string())
                    .await;
                return;
            }
        };
        let timeout = self.approvals.timeout();
        let cancellation = self
            .active
            .as_ref()
            .map(|active| active.cancellation.child_token())
            .expect("active turn checked above");
        let sender = self.command_tx.clone();
        let waiter_task = tokio::spawn(async move {
            let outcome = waiter.wait(timeout, cancellation).await;
            let _ = sender
                .send(ActorInput::ApprovalOutcome {
                    turn_id,
                    approval_id,
                    outcome,
                })
                .await;
        });

        if let Some(active) = self.active.as_mut() {
            active.state = TurnState::AwaitingApproval;
            active.approval_id = Some(approval_id);
            active.approval_waiter = Some(waiter_task);
        }
        if let Err(error) = self.persist_approval_event(
            turn_id,
            EVENT_APPROVAL_REQUESTED,
            serde_json::json!({
                "approval_id": approval_id,
                "tool_call_id": request.tool_call_id,
                "provider_call_id": provider_call_id,
                "tool_name": request.tool_name.clone(),
                "summary": request.summary.clone(),
                "policy_key": request.policy_key.clone(),
                "expires_at": request.expires_at.clone(),
            }),
        ) {
            let _ = self.fail_active(turn_id, "approval", error).await;
            return;
        }
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: self.active.as_ref().map(|active| active.request_id),
            kind: RuntimeEventKind::ApprovalRequested {
                approval_id,
                tool_call_id: request.tool_call_id,
                tool_name: request.tool_name,
                summary: request.summary,
                policy_key: request.policy_key,
                expires_at: request.expires_at,
            },
        });
    }

    pub(super) async fn handle_approval_outcome(
        &mut self,
        turn_id: TurnId,
        approval_id: sagent_types::ApprovalId,
        outcome: ApprovalOutcome,
    ) {
        if !self.is_active_turn(turn_id)
            || self.active.as_ref().and_then(|active| active.approval_id) != Some(approval_id)
        {
            return;
        }
        if matches!(
            outcome,
            ApprovalOutcome::TimedOut | ApprovalOutcome::Cancelled
        ) {
            self.approvals.expire(approval_id);
        }
        if let Some(active) = self.active.as_mut() {
            // waiter 已经把结果送回 mailbox；不能在它自己的 task 中 await join handle。
            active.approval_waiter.take();
            active.approval_id = None;
        }
        match outcome {
            ApprovalOutcome::Approved(decision) => {
                if let Some(active) = self.active.as_mut() {
                    active.state = TurnState::RunningTool;
                    if let Some(batch) = active.pending_tool_batch.as_mut() {
                        // Once 不写入 policy rule；该标记只允许当前游标的调用越过
                        // 一次风险检查，工具完成后会自动清除。
                        batch.approve_current();
                    }
                }
                self.publish(RuntimeEvent {
                    session_id: self.session_id.clone(),
                    turn_id: Some(turn_id),
                    request_id: self.active.as_ref().map(|active| active.request_id),
                    kind: RuntimeEventKind::ApprovalResolved {
                        approval_id,
                        decision,
                    },
                });
                let _ = self.persist_approval_event(
                    turn_id,
                    EVENT_APPROVAL_RESOLVED,
                    serde_json::json!({
                        "approval_id": approval_id,
                        "decision": decision,
                    }),
                );
                self.advance_tool_batch(turn_id).await;
            }
            ApprovalOutcome::Denied => {
                self.publish(RuntimeEvent {
                    session_id: self.session_id.clone(),
                    turn_id: Some(turn_id),
                    request_id: self.active.as_ref().map(|active| active.request_id),
                    kind: RuntimeEventKind::ApprovalResolved {
                        approval_id,
                        decision: ApprovalDecision::Deny,
                    },
                });
                let _ = self.persist_approval_event(
                    turn_id,
                    EVENT_APPROVAL_RESOLVED,
                    serde_json::json!({
                        "approval_id": approval_id,
                        "decision": ApprovalDecision::Deny,
                    }),
                );
                self.fail_current_tool(
                    turn_id,
                    "approval_denied",
                    "approval_denied",
                    "用户拒绝了工具执行".into(),
                )
                .await;
            }
            ApprovalOutcome::TimedOut => {
                self.publish(RuntimeEvent {
                    session_id: self.session_id.clone(),
                    turn_id: Some(turn_id),
                    request_id: self.active.as_ref().map(|active| active.request_id),
                    kind: RuntimeEventKind::ApprovalTimedOut { approval_id },
                });
                let _ = self.persist_approval_event(
                    turn_id,
                    EVENT_APPROVAL_TIMED_OUT,
                    serde_json::json!({ "approval_id": approval_id }),
                );
                self.fail_current_tool(
                    turn_id,
                    "approval_timeout",
                    "approval_timeout",
                    "审批等待超时".into(),
                )
                .await;
            }
            ApprovalOutcome::Cancelled => {}
        }
    }

    pub(super) fn resolve_approval(
        &mut self,
        approval_id: sagent_types::ApprovalId,
        decision: ApprovalDecision,
    ) -> Result<CommandReply, RuntimeError> {
        let active = self.active.as_ref().ok_or(RuntimeError::NoActiveTurn)?;
        if active.state != TurnState::AwaitingApproval {
            return Err(RuntimeError::Approval("当前 Turn 没有等待中的审批".into()));
        }
        self.approvals
            .resolve(&self.session_id, &active.turn_id, approval_id, decision)
            .map_err(|error| RuntimeError::Approval(error.to_string()))?;
        Ok(CommandReply::ApprovalAccepted)
    }
}
