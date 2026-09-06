//! SessionActor 的最小 submit 处理循环。

use std::sync::Arc;
use std::time::Duration;

use sagent_agent::{
    ApprovalDecision, PromptMessage, PromptRole, PromptSnapshot, PromptToolCall, RequestId,
    SessionCommand, SystemPromptParts, TurnState, UserInput,
};
use sagent_provider::ModelProvider;
use sagent_store::{
    EVENT_APPROVAL_REQUESTED, EVENT_APPROVAL_RESOLVED, EVENT_APPROVAL_TIMED_OUT,
    EVENT_TOOL_STARTED, MessageQuery, NewDaemonEvent, NewGeneration, NewMessage, StartTurn, Store,
};
use sagent_tools::{ApprovalPolicyDecision, classify_tool};
use sagent_types::{SessionId, ToolCallId, TurnId};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    RuntimeError,
    active_turn::{ActiveTurn, PendingToolBatch},
    approval::{ApprovalManager, ApprovalOutcome, ApprovalRequest},
    event::{RuntimeEvent, RuntimeEventKind},
    input::{ActorInput, CommandReply, WorkerEvent},
    provider_worker::spawn_provider_worker,
    tool_dispatch::ToolDispatcher,
    tool_worker::{ToolExecutionResult, ToolWorker},
};

const DEFAULT_MODEL_ID: &str = "unconfigured";
const DEFAULT_PROFILE_REVISION: &str = "runtime-v1";
const EMPTY_TOOL_SCHEMA_HASH: &str = "sha256:empty-tools";
const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
const DEFAULT_MAX_TOOL_ROUNDS: u32 = 8;

/// 测试替身或后续 Provider 用来启动 worker 的函数。
pub(crate) type WorkerFactory = Arc<
    dyn Fn(mpsc::Sender<ActorInput>, TurnId, CancellationToken) -> JoinHandle<()> + Send + Sync,
>;

/// 一个 Session 的唯一写入者。
pub(crate) struct SessionActor {
    pub(crate) session_id: SessionId,
    pub(crate) store: Store,
    pub(crate) command_rx: mpsc::Receiver<ActorInput>,
    pub(crate) command_tx: mpsc::Sender<ActorInput>,
    pub(crate) event_tx: broadcast::Sender<RuntimeEvent>,
    pub(crate) active: Option<ActiveTurn>,
    pub(crate) generation: i64,
    pub(crate) worker_factory: Option<WorkerFactory>,
    pub(crate) provider: Option<Arc<dyn ModelProvider>>,
    pub(crate) model: String,
    pub(crate) profile_revision: String,
    pub(crate) clock: fn() -> String,
    pub(crate) approvals: ApprovalManager,
    pub(crate) interactive_approval: bool,
    pub(crate) tool_dispatcher: Option<ToolDispatcher>,
    pub(crate) tool_worker: Option<Arc<ToolWorker>>,
    pub(crate) max_tool_rounds: u32,
    /// Actor 启动时从 Store 得到的 fail-closed 恢复计划；在处理第一条外部命令前
    /// 执行，避免新 Turn 与遗留 running Turn 并存。
    pub(crate) startup_recovery: Result<Option<crate::recovery::RecoveryPlan>, String>,
}

impl SessionActor {
    /// 创建不启动 worker 的 Actor；单测可直接驱动提交边界和 mailbox 状态机。
    pub(crate) fn new(
        session_id: SessionId,
        store: Store,
        command_rx: mpsc::Receiver<ActorInput>,
        command_tx: mpsc::Sender<ActorInput>,
        event_tx: broadcast::Sender<RuntimeEvent>,
    ) -> Self {
        let startup_recovery = crate::recovery::plan_recovery(&store, &session_id);
        Self {
            session_id,
            store,
            command_rx,
            command_tx,
            event_tx,
            active: None,
            generation: 0,
            worker_factory: None,
            provider: None,
            model: DEFAULT_MODEL_ID.to_owned(),
            profile_revision: DEFAULT_PROFILE_REVISION.to_owned(),
            clock: utc_now,
            approvals: ApprovalManager::new(DEFAULT_APPROVAL_TIMEOUT),
            interactive_approval: true,
            tool_dispatcher: None,
            tool_worker: None,
            max_tool_rounds: DEFAULT_MAX_TOOL_ROUNDS,
            startup_recovery,
        }
    }

    /// 为测试或后续 Provider 注入受监管的 worker 工厂和时钟。
    pub(crate) fn with_worker_factory(
        mut self,
        worker_factory: WorkerFactory,
        clock: fn() -> String,
    ) -> Self {
        self.worker_factory = Some(worker_factory);
        self.clock = clock;
        self
    }

    /// 覆盖审批等待时长；生产环境由 Runtime 配置注入，测试可使用很短的超时。
    pub(crate) fn with_approval_timeout(mut self, timeout: Duration) -> Self {
        self.approvals = ApprovalManager::new(timeout);
        self
    }

    /// 注入真实 Provider；Provider worker 仍由 Actor 监管，不能直接写 Store。
    pub(crate) fn with_provider(
        mut self,
        provider: Arc<dyn ModelProvider>,
        model: impl Into<String>,
        profile_revision: impl Into<String>,
    ) -> Self {
        self.provider = Some(provider);
        self.model = model.into();
        self.profile_revision = profile_revision.into();
        self
    }

    /// 注入当前 generation 可见的工具 registry；未注入时保持 fail-closed。
    pub(crate) fn with_tool_dispatcher(mut self, dispatcher: ToolDispatcher) -> Self {
        self.tool_dispatcher = Some(dispatcher);
        self
    }

    /// 注入具体工具执行器；工具 worker 不能直接写 Store。
    pub(crate) fn with_tool_worker(mut self, worker: ToolWorker) -> Self {
        self.tool_worker = Some(Arc::new(worker));
        self
    }

    /// 限制单个 Turn 的工具回环次数，避免 Provider 重复发起同一工具调用。
    pub(crate) fn with_max_tool_rounds(mut self, limit: u32) -> Self {
        self.max_tool_rounds = limit.max(1);
        self
    }

    /// 按顺序消费 mailbox；Store 只在这个循环所属的 Actor 中被写入。
    pub(crate) async fn run(mut self) {
        self.apply_startup_recovery();
        while let Some(input) = self.command_rx.recv().await {
            if self.handle_input(input).await {
                break;
            }
        }
    }

    /// 将进程重启前遗留的 running Turn 安全收口。任何未完成工具都只会获得描述
    /// “未执行/结果未知”的持久化结果，绝不在恢复阶段重新启动工具或 Provider。
    fn apply_startup_recovery(&mut self) {
        let recovery = std::mem::replace(&mut self.startup_recovery, Ok(None));
        let plan = match recovery {
            Ok(Some(plan)) => plan,
            Ok(None) => return,
            Err(reason) => {
                self.publish(RuntimeEvent {
                    session_id: self.session_id.clone(),
                    turn_id: None,
                    request_id: None,
                    kind: RuntimeEventKind::TurnFailed {
                        reason: format!("读取重启恢复状态失败：{reason}"),
                    },
                });
                return;
            }
        };
        for recovered in plan.unresolved_tools {
            let result = ToolExecutionResult {
                call_id: recovered.call_id,
                name: recovered.name,
                ok: false,
                content: recovered.message.to_owned(),
                truncated: false,
                exit_code: None,
                error_kind: Some(recovered.error_kind.to_owned()),
            };
            if let Err(error) = self.persist_tool_result(plan.turn_id, result) {
                self.publish(RuntimeEvent {
                    session_id: self.session_id.clone(),
                    turn_id: Some(plan.turn_id),
                    request_id: None,
                    kind: RuntimeEventKind::TurnFailed {
                        reason: format!("写入重启恢复结果失败：{error}"),
                    },
                });
                return;
            }
        }
        let reason = "Runtime 重启导致未完成 Turn 安全终止".to_owned();
        if let Err(error) =
            self.store
                .fail_turn(&plan.turn_id, "runtime_restarted", &reason, &(self.clock)())
        {
            self.publish(RuntimeEvent {
                session_id: self.session_id.clone(),
                turn_id: Some(plan.turn_id),
                request_id: None,
                kind: RuntimeEventKind::TurnFailed {
                    reason: format!("写入重启终态失败：{error}"),
                },
            });
            return;
        }
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(plan.turn_id),
            request_id: None,
            kind: RuntimeEventKind::TurnFailed { reason },
        });
    }

    async fn handle_input(&mut self, input: ActorInput) -> bool {
        match input {
            ActorInput::Command { command, reply_to } => {
                let is_close = matches!(command, SessionCommand::Close);
                let result = self.handle_command(command).await;
                let _ = reply_to.send(result);
                is_close
            }
            ActorInput::Worker(event) => {
                self.handle_worker_event(event).await;
                false
            }
            ActorInput::WorkerExited { turn_id, result } => {
                self.handle_worker_exited(turn_id, result).await;
                false
            }
            ActorInput::ApprovalRequired { turn_id, request } => {
                self.handle_approval_required(turn_id, request).await;
                false
            }
            ActorInput::ApprovalOutcome {
                turn_id,
                approval_id,
                outcome,
            } => {
                self.handle_approval_outcome(turn_id, approval_id, outcome)
                    .await;
                false
            }
        }
    }

    async fn handle_command(
        &mut self,
        command: SessionCommand,
    ) -> Result<CommandReply, RuntimeError> {
        match command {
            SessionCommand::SubmitPrompt { request_id, input } => {
                self.submit_prompt(request_id, input)
            }
            SessionCommand::Close => {
                // Close 在空闲会话上是幂等的；有 active Turn 时才需要先收口。
                if self.active.is_some() {
                    self.interrupt_active("session closing").await?;
                }
                Ok(CommandReply::Closed)
            }
            SessionCommand::Interrupt { .. } => self.interrupt_active("user interrupt").await,
            SessionCommand::ResolveApproval {
                approval_id,
                decision,
            } => self.resolve_approval(approval_id, decision),
            SessionCommand::Resume { client } => {
                self.interactive_approval = client.interactive_approval;
                Ok(CommandReply::Resumed)
            }
        }
    }

    fn submit_prompt(
        &mut self,
        request_id: RequestId,
        input: UserInput,
    ) -> Result<CommandReply, RuntimeError> {
        if self.active.is_some() {
            return Err(RuntimeError::Busy {
                session_id: self.session_id.clone(),
            });
        }

        let turn_id = TurnId::new();
        let system = SystemPromptParts {
            identity: "你是 Sagent。".into(),
            instructions: "请准确回答用户问题。".into(),
            environment: "运行时：sagent-runtime。".into(),
            ..SystemPromptParts::default()
        };
        let messages = vec![
            PromptMessage::new(PromptRole::System, system.render()),
            PromptMessage::new(PromptRole::User, input.as_str()),
        ];
        let snapshot = PromptSnapshot::new(self.session_id.clone(), turn_id, &system, messages)
            .map_err(|error| RuntimeError::InvalidLifecycle(error.to_string()))?;
        let system_hash = snapshot.system_prompt_hash.clone();
        let (tool_schema_hash, tools) = self.tool_schema()?;

        self.ensure_generation(&system_hash, &tool_schema_hash)?;

        let timestamp = (self.clock)();
        let message = NewMessage::new(
            self.session_id.clone(),
            "user",
            input.as_str(),
            timestamp.clone(),
        );
        let persisted_message_id = self
            .store
            .begin_turn(
                &StartTurn {
                    turn_id,
                    session_id: self.session_id.clone(),
                    generation: self.generation,
                    started_at: timestamp,
                },
                &message,
            )
            .map_err(|error| RuntimeError::Persistence(error.to_string()))?;

        let cancellation = CancellationToken::new();
        let worker = if let Some(provider) = self.provider.clone() {
            Some(spawn_provider_worker(
                provider,
                snapshot,
                self.model.clone(),
                request_id,
                tools,
                self.command_tx.clone(),
                cancellation.child_token(),
            ))
        } else {
            self.worker_factory.as_ref().map(|factory| {
                factory(self.command_tx.clone(), turn_id, cancellation.child_token())
            })
        };
        let worker_abort = worker.as_ref().map(JoinHandle::abort_handle);
        let worker = worker.map(|worker| {
            let sender = self.command_tx.clone();
            tokio::spawn(async move {
                let result = worker
                    .await
                    .map_err(|error| crate::input::WorkerFailure(error.to_string()));
                // 退出事实不能因为 mailbox 暂时拥塞而丢失；stop_worker 会在
                // Actor 已经决定终态时主动 abort 这个监控任务，避免等待发送造成死锁。
                let _ = sender
                    .send(ActorInput::WorkerExited { turn_id, result })
                    .await;
            })
        });
        self.active = Some(ActiveTurn {
            turn_id,
            request_id,
            generation: self.generation,
            tool_rounds: 0,
            provider_exit_credits: 0,
            system,
            state: TurnState::Prompting,
            cancellation,
            worker,
            worker_abort,
            tool_task: None,
            approval_waiter: None,
            approval_id: None,
            pending_tool_batch: None,
            terminal: false,
        });

        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(request_id),
            kind: RuntimeEventKind::PromptAccepted,
        });
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(request_id),
            kind: RuntimeEventKind::UserMessagePersisted {
                message_id: persisted_message_id,
            },
        });

        Ok(CommandReply::Accepted { turn_id })
    }

    fn ensure_generation(
        &mut self,
        system_hash: &str,
        tool_schema_hash: &str,
    ) -> Result<(), RuntimeError> {
        match self
            .store
            .get_generation(&self.session_id, self.generation)
            .map_err(|error| RuntimeError::Persistence(error.to_string()))?
        {
            Some(generation)
                if generation.system_hash == system_hash
                    && generation.tool_schema_hash == tool_schema_hash => {}
            Some(_) => return Err(RuntimeError::RequiresTransition),
            None => self
                .store
                .create_generation(&NewGeneration {
                    session_id: self.session_id.clone(),
                    generation: self.generation,
                    system_hash: system_hash.to_owned(),
                    tool_schema_hash: tool_schema_hash.to_owned(),
                    model_id: self.model.clone(),
                    profile_revision: self.profile_revision.clone(),
                    created_at: (self.clock)(),
                })
                .map_err(|error| RuntimeError::Persistence(error.to_string()))?,
        }
        Ok(())
    }

    /// 返回本 generation 对模型可见的稳定工具 schema 和 fingerprint。
    fn tool_schema(&self) -> Result<(String, Vec<serde_json::Value>), RuntimeError> {
        let Some(dispatcher) = self.tool_dispatcher.as_ref() else {
            return Ok((EMPTY_TOOL_SCHEMA_HASH.to_owned(), Vec::new()));
        };
        let schema = dispatcher
            .registry()
            .model_schema()
            .map_err(|error| RuntimeError::InvalidLifecycle(error.to_string()))?;
        let tools = schema.as_array().cloned().ok_or_else(|| {
            RuntimeError::InvalidLifecycle("ToolRegistry schema 必须是 JSON 数组".into())
        })?;
        let hash = dispatcher
            .registry()
            .schema_hash()
            .map_err(|error| RuntimeError::InvalidLifecycle(error.to_string()))?;
        Ok((hash, tools))
    }

    /// 从 Store 的活动模型上下文恢复下一轮 Provider 所需的 PromptSnapshot。
    ///
    /// 不能读取 display history：压缩前的归档消息和回退分支都不属于模型上下文。
    fn build_prompt_snapshot(&self, turn_id: TurnId) -> Result<PromptSnapshot, RuntimeError> {
        let system = self
            .active
            .as_ref()
            .filter(|active| active.turn_id == turn_id)
            .map(|active| active.system.clone())
            .ok_or(RuntimeError::NoActiveTurn)?;
        let stored = self
            .store
            .get_messages_for_model(&self.session_id, &MessageQuery::default())
            .map_err(|error| RuntimeError::Persistence(error.to_string()))?;
        let mut messages = vec![PromptMessage::new(PromptRole::System, system.render())];
        for message in stored {
            let prompt = match message.role.as_str() {
                "user" => PromptMessage::new(PromptRole::User, message.content),
                "assistant" => {
                    let mut prompt = PromptMessage::new(PromptRole::Assistant, message.content);
                    if let Some(tool_calls) = message.tool_calls {
                        prompt.tool_calls = serde_json::from_str::<Vec<PromptToolCall>>(
                            &tool_calls,
                        )
                        .map_err(|error| {
                            RuntimeError::InvalidLifecycle(format!(
                                "解析 assistant tool_calls 失败：{error}"
                            ))
                        })?;
                    }
                    prompt
                }
                "tool" => PromptMessage::tool(
                    message.content,
                    message.tool_call_id.ok_or_else(|| {
                        RuntimeError::InvalidLifecycle("tool 消息缺少 tool_call_id".into())
                    })?,
                ),
                role => {
                    return Err(RuntimeError::InvalidLifecycle(format!(
                        "模型上下文包含不支持的消息角色：{role}"
                    )));
                }
            };
            messages.push(prompt);
        }
        PromptSnapshot::new(self.session_id.clone(), turn_id, &system, messages)
            .map_err(|error| RuntimeError::InvalidLifecycle(error.to_string()))
    }

    /// 用已持久化的 user、assistant(tool_calls) 和 tool messages 启动下一轮模型调用。
    fn start_next_provider_round(&mut self, turn_id: TurnId) -> Result<(), RuntimeError> {
        let provider = self.provider.clone().ok_or_else(|| {
            RuntimeError::InvalidLifecycle("Provider 未配置，不能继续工具回环".into())
        })?;
        let snapshot = self.build_prompt_snapshot(turn_id)?;
        let (_, tools) = self.tool_schema()?;
        let (request_id, cancellation) = self
            .active
            .as_ref()
            .filter(|active| active.turn_id == turn_id)
            .map(|active| (active.request_id, active.cancellation.child_token()))
            .ok_or(RuntimeError::NoActiveTurn)?;
        let worker = spawn_provider_worker(
            provider,
            snapshot,
            self.model.clone(),
            request_id,
            tools,
            self.command_tx.clone(),
            cancellation,
        );
        let worker_abort = worker.abort_handle();
        let sender = self.command_tx.clone();
        let monitor = tokio::spawn(async move {
            let result = worker
                .await
                .map_err(|error| crate::input::WorkerFailure(error.to_string()));
            let _ = sender
                .send(ActorInput::WorkerExited { turn_id, result })
                .await;
        });
        let active = self
            .active
            .as_mut()
            .filter(|active| active.turn_id == turn_id)
            .ok_or(RuntimeError::NoActiveTurn)?;
        active.worker = Some(monitor);
        active.worker_abort = Some(worker_abort);
        active.state = TurnState::AwaitingModel;
        Ok(())
    }

    /// 推进当前 Provider 产生的工具批次；每次只启动一个工具，因而能在危险调用前
    /// 安全暂停并在审批后从同一位置恢复。
    async fn advance_tool_batch(&mut self, turn_id: TurnId) {
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
        self.store
            .append_event(&NewDaemonEvent {
                session_id: self.session_id.clone(),
                turn_id: Some(turn_id),
                event_type: EVENT_TOOL_STARTED.to_owned(),
                payload: serde_json::json!({
                    "provider_call_id": plan.call.call_id,
                    "tool_name": plan.call.name,
                }),
                created_at: (self.clock)(),
            })
            .map(|_| ())
            .map_err(|error| RuntimeError::Persistence(error.to_string()))
    }

    /// 将一个工具结果和其事件作为同一 Actor 操作提交；调用方随后才能推进游标。
    fn persist_tool_result(
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
    async fn handle_approval_required(&mut self, turn_id: TurnId, request: ApprovalRequest) {
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

    async fn handle_approval_outcome(
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

    fn resolve_approval(
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

    async fn handle_worker_event(&mut self, event: WorkerEvent) {
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

    async fn handle_worker_exited(
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

    async fn interrupt_active(&mut self, reason: &str) -> Result<CommandReply, RuntimeError> {
        let turn_id = self
            .active
            .as_ref()
            .map(|active| active.turn_id)
            .ok_or(RuntimeError::NoActiveTurn)?;
        self.interrupt_active_for_turn(turn_id, reason).await
    }

    async fn interrupt_active_for_turn(
        &mut self,
        turn_id: TurnId,
        reason: &str,
    ) -> Result<CommandReply, RuntimeError> {
        let Some(mut active) = self.take_active(turn_id) else {
            return Err(RuntimeError::NoActiveTurn);
        };
        if active.terminal {
            self.active = Some(active);
            return Err(RuntimeError::NoActiveTurn);
        }
        active.cancellation.cancel();
        self.approvals.cancel_for_turn(&turn_id);
        Self::stop_worker(&mut active).await;
        let timestamp = (self.clock)();
        if let Err(error) = self.store.interrupt_turn(&turn_id, reason, &timestamp) {
            self.active = Some(active);
            return Err(RuntimeError::Persistence(error.to_string()));
        }
        active.terminal = true;
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(active.request_id),
            kind: RuntimeEventKind::TurnInterrupted,
        });
        Ok(CommandReply::Interrupted)
    }

    async fn complete_active(&mut self, turn_id: TurnId, text: String) -> Result<(), RuntimeError> {
        let Some(mut active) = self.take_active(turn_id) else {
            return Ok(());
        };
        if active.terminal || active.cancellation.is_cancelled() {
            self.active = Some(active);
            return Ok(());
        }
        let timestamp = (self.clock)();
        let message = NewMessage::new(
            self.session_id.clone(),
            "assistant",
            text,
            timestamp.clone(),
        );
        let message_id = match self.store.complete_turn(&turn_id, &message, &timestamp) {
            Ok(message_id) => message_id,
            Err(error) => {
                self.active = Some(active);
                return Err(RuntimeError::Persistence(error.to_string()));
            }
        };
        active.terminal = true;
        self.approvals.cancel_for_turn(&turn_id);
        Self::stop_worker(&mut active).await;
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(active.request_id),
            kind: RuntimeEventKind::FinalMessagePersisted { message_id },
        });
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(active.request_id),
            kind: RuntimeEventKind::TurnCompleted,
        });
        Ok(())
    }

    async fn fail_active(
        &mut self,
        turn_id: TurnId,
        category: &str,
        reason: String,
    ) -> Result<(), RuntimeError> {
        let Some(mut active) = self.take_active(turn_id) else {
            return Ok(());
        };
        if active.terminal {
            self.active = Some(active);
            return Ok(());
        }
        active.cancellation.cancel();
        self.approvals.cancel_for_turn(&turn_id);
        Self::stop_worker(&mut active).await;
        let timestamp = (self.clock)();
        if let Err(error) = self
            .store
            .fail_turn(&turn_id, category, &reason, &timestamp)
        {
            self.active = Some(active);
            return Err(RuntimeError::Persistence(error.to_string()));
        }
        active.terminal = true;
        self.publish(RuntimeEvent {
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(active.request_id),
            kind: RuntimeEventKind::TurnFailed { reason },
        });
        Ok(())
    }

    fn take_active(&mut self, turn_id: TurnId) -> Option<ActiveTurn> {
        if self.is_active_turn(turn_id) {
            self.active.take()
        } else {
            None
        }
    }

    async fn stop_worker(active: &mut ActiveTurn) {
        // 先让工具观察 cancellation，自行执行 terminal 进程树清理；只有超时才
        // abort task，避免 Tokio abort 直接跳过 TerminalExecutor 的 finally 路径。
        if let Some(mut tool_task) = active.tool_task.take()
            && tokio::time::timeout(Duration::from_secs(3), &mut tool_task)
                .await
                .is_err()
        {
            tool_task.abort();
            let _ = tool_task.await;
        }
        if let Some(abort) = active.worker_abort.take() {
            abort.abort();
        }
        if let Some(worker) = active.worker.take() {
            // 监控任务可能正阻塞在有界 mailbox 的 send 上，先 abort 它再等待。
            worker.abort();
            let _ = worker.await;
        }
        if let Some(waiter) = active.approval_waiter.take() {
            waiter.abort();
            let _ = waiter.await;
        }
        active.approval_id = None;
    }

    fn is_cancelled(&self, turn_id: TurnId) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.turn_id == turn_id && active.cancellation.is_cancelled())
    }

    fn is_active_turn(&self, turn_id: TurnId) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.turn_id == turn_id)
    }

    fn publish(&self, event: RuntimeEvent) {
        let _ = self.event_tx.send(event);
    }

    fn persist_approval_event(
        &mut self,
        turn_id: TurnId,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<(), String> {
        self.store
            .append_event(&NewDaemonEvent {
                session_id: self.session_id.clone(),
                turn_id: Some(turn_id),
                event_type: event_type.to_owned(),
                payload,
                created_at: (self.clock)(),
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

pub(crate) fn utc_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    format!("{seconds:020}")
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, sync::Arc};

    use sagent_store::{EventQuery, MessageQuery, NewSession, Store};
    use tokio::sync::{broadcast, mpsc, oneshot};
    use tokio_util::sync::CancellationToken;

    use super::SessionActor;
    use crate::{
        actor::WorkerFactory,
        approval::ApprovalRequest,
        event::RuntimeEventKind,
        input::{ActorInput, CommandReply, WorkerEvent},
    };
    use sagent_agent::{RequestId, SessionCommand, UserInput};
    use sagent_types::{EventSequence, SessionId, ToolCallId, TurnId};

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sagent-runtime-actor-{name}-{}.db",
            std::process::id()
        ))
    }

    fn prepare_store(path: &std::path::Path, session_id: &SessionId) -> Store {
        let mut store = Store::open_readwrite(path).expect("应能打开测试数据库");
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: Some("test".into()),
                model: Some("test-model".into()),
                title: None,
                started_at: "2026-09-03T00:00:00Z".into(),
            })
            .expect("应能创建测试会话");
        store
    }

    fn fixed_clock() -> String {
        "2026-09-03T00:00:00Z".into()
    }

    #[tokio::test]
    async fn submit_persists_before_publishing_acceptance() {
        let path = test_path("submit");
        let session_id = SessionId::new("actor-submit");
        let store = prepare_store(&path, &session_id);
        let (sender, receiver) = mpsc::channel(8);
        let (events, mut event_receiver) = broadcast::channel(8);
        let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
        let actor_task = tokio::spawn(actor.run());
        let request_id = RequestId::new();
        let (reply_to, reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::SubmitPrompt {
                    request_id,
                    input: UserInput::new("你好").expect("输入有效"),
                },
                reply_to,
            })
            .await
            .expect("命令应能投递");

        let response = reply.await.expect("Actor 应返回结果").expect("提交应成功");
        let turn_id = match response {
            CommandReply::Accepted { turn_id } => turn_id,
            _ => panic!("应返回 Accepted"),
        };
        let accepted = event_receiver.recv().await.expect("应收到 accepted");
        let persisted = event_receiver.recv().await.expect("应收到 persisted");
        assert!(matches!(accepted.kind, RuntimeEventKind::PromptAccepted));
        assert!(matches!(
            persisted.kind,
            RuntimeEventKind::UserMessagePersisted { .. }
        ));
        assert_eq!(accepted.turn_id, Some(turn_id));

        let (close_tx, close_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::Close,
                reply_to: close_tx,
            })
            .await
            .expect("关闭命令应能投递");
        assert!(matches!(
            close_reply
                .await
                .expect("应返回关闭结果")
                .expect("关闭应成功"),
            CommandReply::Closed
        ));
        actor_task.await.expect("Actor 不应 panic");

        let store = Store::open_readonly(&path).expect("应能重新打开数据库");
        let messages = store
            .get_messages_for_display(&session_id, &MessageQuery::default())
            .expect("应能读取消息");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "你好");
        assert!(
            store
                .get_generation(&session_id, 0)
                .expect("应能读取 generation")
                .is_some()
        );
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn second_submit_is_rejected_without_a_second_message() {
        let path = test_path("busy");
        let session_id = SessionId::new("actor-busy");
        let store = prepare_store(&path, &session_id);
        let (sender, receiver) = mpsc::channel(8);
        let (events, _event_receiver) = broadcast::channel(8);
        let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
        let actor_task = tokio::spawn(actor.run());

        for text in ["第一条", "第二条"] {
            let (reply_to, reply) = oneshot::channel();
            sender
                .send(ActorInput::Command {
                    command: SessionCommand::SubmitPrompt {
                        request_id: RequestId::new(),
                        input: UserInput::new(text).expect("输入有效"),
                    },
                    reply_to,
                })
                .await
                .expect("命令应能投递");
            let result = reply.await.expect("Actor 应返回结果");
            if text == "第一条" {
                assert!(matches!(result, Ok(CommandReply::Accepted { .. })));
            } else {
                assert!(matches!(result, Err(crate::RuntimeError::Busy { .. })));
            }
        }

        let (close_tx, close_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::Close,
                reply_to: close_tx,
            })
            .await
            .expect("关闭命令应能投递");
        close_reply
            .await
            .expect("应返回关闭结果")
            .expect("关闭应成功");
        actor_task.await.expect("Actor 不应 panic");

        let store = Store::open_readonly(&path).expect("应能重新打开数据库");
        assert_eq!(
            store
                .get_messages_for_display(&session_id, &MessageQuery::default())
                .expect("应能读取消息")
                .len(),
            1
        );
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn final_text_is_persisted_before_completion_events() {
        let path = test_path("final");
        let session_id = SessionId::new("actor-final");
        let store = prepare_store(&path, &session_id);
        let (sender, receiver) = mpsc::channel(8);
        let (events, mut event_receiver) = broadcast::channel(16);
        let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
        let actor_task = tokio::spawn(actor.run());

        let (reply_to, reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::SubmitPrompt {
                    request_id: RequestId::new(),
                    input: UserInput::new("生成答案").expect("输入有效"),
                },
                reply_to,
            })
            .await
            .expect("提交应能投递");
        let turn_id = match reply.await.expect("应返回结果").expect("提交应成功") {
            CommandReply::Accepted { turn_id } => turn_id,
            _ => panic!("应返回 Accepted"),
        };
        let _ = event_receiver.recv().await;
        let _ = event_receiver.recv().await;

        sender
            .send(ActorInput::Worker(WorkerEvent::FinalText {
                turn_id,
                text: "这是最终答案".into(),
            }))
            .await
            .expect("最终事件应能投递");
        let persisted = event_receiver.recv().await.expect("应收到持久化事件");
        assert!(matches!(
            persisted.kind,
            RuntimeEventKind::FinalMessagePersisted { .. }
        ));

        // 收到完成消息确认时，Store 中的 assistant 消息和持久化事件已经可读。
        let store = Store::open_readonly(&path).expect("应能重新打开数据库");
        let messages = store
            .get_messages_for_display(&session_id, &MessageQuery::default())
            .expect("应能读取消息");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content, "这是最终答案");
        let persisted_events = store
            .events_since(&EventQuery {
                session_id: session_id.clone(),
                after_sequence: EventSequence::new(0).expect("序号有效"),
                limit: 200,
            })
            .expect("应能读取持久化事件");
        assert!(
            persisted_events
                .iter()
                .any(|event| event.event_type == "turn.completed")
        );

        let completed = event_receiver.recv().await.expect("应收到完成事件");
        assert!(matches!(completed.kind, RuntimeEventKind::TurnCompleted));

        let (close_tx, close_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::Close,
                reply_to: close_tx,
            })
            .await
            .expect("关闭应能投递");
        assert!(matches!(
            close_reply
                .await
                .expect("应返回关闭结果")
                .expect("关闭应成功"),
            CommandReply::Closed
        ));
        actor_task.await.expect("Actor 不应 panic");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn interrupt_marks_turn_without_creating_assistant_message() {
        let path = test_path("interrupt");
        let session_id = SessionId::new("actor-interrupt");
        let store = prepare_store(&path, &session_id);
        let (sender, receiver) = mpsc::channel(8);
        let (events, mut event_receiver) = broadcast::channel(16);
        let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
        let actor_task = tokio::spawn(actor.run());

        let (submit_tx, submit_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::SubmitPrompt {
                    request_id: RequestId::new(),
                    input: UserInput::new("中断我").expect("输入有效"),
                },
                reply_to: submit_tx,
            })
            .await
            .expect("提交应能投递");
        let _turn_id = match submit_reply.await.expect("应返回结果").expect("提交应成功")
        {
            CommandReply::Accepted { turn_id } => turn_id,
            _ => panic!("应返回 Accepted"),
        };
        let _ = event_receiver.recv().await;
        let _ = event_receiver.recv().await;

        let (interrupt_tx, interrupt_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::Interrupt {
                    request_id: RequestId::new(),
                },
                reply_to: interrupt_tx,
            })
            .await
            .expect("中断应能投递");
        assert!(matches!(
            interrupt_reply
                .await
                .expect("应返回中断结果")
                .expect("中断应成功"),
            CommandReply::Interrupted
        ));
        let interrupted = event_receiver.recv().await.expect("应收到中断事件");
        assert!(matches!(
            interrupted.kind,
            RuntimeEventKind::TurnInterrupted
        ));

        let store = Store::open_readonly(&path).expect("应能重新打开数据库");
        let messages = store
            .get_messages_for_display(&session_id, &MessageQuery::default())
            .expect("应能读取消息");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");

        let (close_tx, close_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::Close,
                reply_to: close_tx,
            })
            .await
            .expect("关闭应能投递");
        close_reply
            .await
            .expect("应返回关闭结果")
            .expect("关闭应成功");
        actor_task.await.expect("Actor 不应 panic");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn failed_worker_marks_turn_failed_without_assistant_message() {
        let path = test_path("failed");
        let session_id = SessionId::new("actor-failed");
        let store = prepare_store(&path, &session_id);
        let (sender, receiver) = mpsc::channel(8);
        let (events, mut event_receiver) = broadcast::channel(16);
        let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
        let actor_task = tokio::spawn(actor.run());

        let (submit_tx, submit_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::SubmitPrompt {
                    request_id: RequestId::new(),
                    input: UserInput::new("触发失败").expect("输入有效"),
                },
                reply_to: submit_tx,
            })
            .await
            .expect("提交应能投递");
        let turn_id = match submit_reply.await.expect("应返回结果").expect("提交应成功") {
            CommandReply::Accepted { turn_id } => turn_id,
            _ => panic!("应返回 Accepted"),
        };
        let _ = event_receiver.recv().await;
        let _ = event_receiver.recv().await;

        sender
            .send(ActorInput::Worker(WorkerEvent::Failed {
                turn_id,
                reason: "provider unavailable".into(),
            }))
            .await
            .expect("失败事件应能投递");
        let failed = event_receiver.recv().await.expect("应收到失败事件");
        assert!(matches!(
            failed.kind,
            RuntimeEventKind::TurnFailed { ref reason } if reason == "provider unavailable"
        ));

        let store = Store::open_readonly(&path).expect("应能重新打开数据库");
        let messages = store
            .get_messages_for_display(&session_id, &MessageQuery::default())
            .expect("应能读取消息");
        assert_eq!(messages.len(), 1);

        let (close_tx, close_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::Close,
                reply_to: close_tx,
            })
            .await
            .expect("关闭应能投递");
        close_reply
            .await
            .expect("应返回关闭结果")
            .expect("关闭应成功");
        actor_task.await.expect("Actor 不应 panic");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn worker_panic_is_converted_to_failed_turn() {
        let path = test_path("panic");
        let session_id = SessionId::new("actor-panic");
        let store = prepare_store(&path, &session_id);
        let (sender, receiver) = mpsc::channel(8);
        let (events, mut event_receiver) = broadcast::channel(16);
        let factory: WorkerFactory = Arc::new(
            |_sender: mpsc::Sender<ActorInput>, _turn_id: TurnId, _token: CancellationToken| {
                tokio::spawn(async { panic!("worker panic") })
            },
        );
        let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events)
            .with_worker_factory(factory, fixed_clock);
        let actor_task = tokio::spawn(actor.run());

        let (submit_tx, submit_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::SubmitPrompt {
                    request_id: RequestId::new(),
                    input: UserInput::new("触发 panic").expect("输入有效"),
                },
                reply_to: submit_tx,
            })
            .await
            .expect("提交应能投递");
        submit_reply.await.expect("应返回结果").expect("提交应成功");
        let _ = event_receiver.recv().await;
        let _ = event_receiver.recv().await;
        let failed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let event = event_receiver.recv().await.expect("事件通道不应关闭");
                if matches!(event.kind, RuntimeEventKind::TurnFailed { .. }) {
                    break event;
                }
            }
        })
        .await
        .expect("panic 应在超时前转换为失败");
        assert!(matches!(failed.kind, RuntimeEventKind::TurnFailed { .. }));

        let store = Store::open_readonly(&path).expect("应能重新打开数据库");
        assert_eq!(
            store
                .get_messages_for_display(&session_id, &MessageQuery::default())
                .expect("应能读取消息")
                .len(),
            1
        );
        let (close_tx, close_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::Close,
                reply_to: close_tx,
            })
            .await
            .expect("关闭应能投递");
        close_reply
            .await
            .expect("应返回关闭结果")
            .expect("关闭应成功");
        actor_task.await.expect("Actor 不应 panic");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn model_delta_is_realtime_only_and_not_persisted() {
        let path = test_path("delta-only");
        let session_id = SessionId::new("actor-delta-only");
        let store = prepare_store(&path, &session_id);
        let (sender, receiver) = mpsc::channel(8);
        let (events, mut event_receiver) = broadcast::channel(16);
        let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
        let actor_task = tokio::spawn(actor.run());

        let (submit_tx, submit_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::SubmitPrompt {
                    request_id: RequestId::new(),
                    input: UserInput::new("流式输出").expect("输入有效"),
                },
                reply_to: submit_tx,
            })
            .await
            .expect("提交应能投递");
        let turn_id = match submit_reply.await.expect("应返回结果").expect("提交应成功") {
            CommandReply::Accepted { turn_id } => turn_id,
            _ => panic!("应返回 Accepted"),
        };
        let _ = event_receiver.recv().await;
        let _ = event_receiver.recv().await;
        let before = Store::open_readonly(&path)
            .expect("应能打开只读 Store")
            .latest_event_sequence(&session_id)
            .expect("应能读取事件序号")
            .expect("提交后应有事件");

        sender
            .send(ActorInput::Worker(WorkerEvent::TextDelta {
                turn_id,
                text: "实时片段".into(),
            }))
            .await
            .expect("delta 应能投递");
        let delta = event_receiver.recv().await.expect("应收到 delta");
        assert!(matches!(
            delta.kind,
            RuntimeEventKind::ModelTextDelta { .. }
        ));

        let store = Store::open_readonly(&path).expect("应能重新打开数据库");
        let after = store
            .latest_event_sequence(&session_id)
            .expect("应能读取事件序号")
            .expect("提交后应有事件");
        assert_eq!(before, after, "delta 不应写入 daemon_events");

        let events = store
            .events_since(&EventQuery {
                session_id: session_id.clone(),
                after_sequence: EventSequence::new(0).expect("序号有效"),
                limit: 200,
            })
            .expect("应能读取事件历史");
        assert!(
            events
                .iter()
                .all(|event| event.event_type != "model.text.delta")
        );

        let (close_tx, close_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::Close,
                reply_to: close_tx,
            })
            .await
            .expect("关闭应能投递");
        close_reply
            .await
            .expect("应返回关闭结果")
            .expect("关闭应成功");
        actor_task.await.expect("Actor 不应 panic");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn final_wins_over_a_later_interrupt_and_late_command_has_no_side_effect() {
        let path = test_path("race");
        let session_id = SessionId::new("actor-race");
        let store = prepare_store(&path, &session_id);
        let (sender, receiver) = mpsc::channel(8);
        let (events, mut event_receiver) = broadcast::channel(16);
        let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
        let actor_task = tokio::spawn(actor.run());

        let (submit_tx, submit_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::SubmitPrompt {
                    request_id: RequestId::new(),
                    input: UserInput::new("竞态测试").expect("输入有效"),
                },
                reply_to: submit_tx,
            })
            .await
            .expect("提交应能投递");
        let turn_id = match submit_reply.await.expect("应返回结果").expect("提交应成功") {
            CommandReply::Accepted { turn_id } => turn_id,
            _ => panic!("应返回 Accepted"),
        };
        let _ = event_receiver.recv().await;
        let _ = event_receiver.recv().await;

        sender
            .send(ActorInput::Worker(WorkerEvent::FinalText {
                turn_id,
                text: "先到的最终结果".into(),
            }))
            .await
            .expect("最终事件应能投递");
        let (interrupt_tx, interrupt_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::Interrupt {
                    request_id: RequestId::new(),
                },
                reply_to: interrupt_tx,
            })
            .await
            .expect("中断应能投递");

        let _ = event_receiver.recv().await;
        let completed = event_receiver.recv().await.expect("应收到完成事件");
        assert!(matches!(completed.kind, RuntimeEventKind::TurnCompleted));
        assert!(matches!(
            interrupt_reply.await.expect("应返回中断结果"),
            Err(crate::RuntimeError::NoActiveTurn)
        ));

        let store = Store::open_readonly(&path).expect("应能重新打开数据库");
        let messages = store
            .get_messages_for_display(&session_id, &MessageQuery::default())
            .expect("应能读取消息");
        assert_eq!(messages.len(), 2, "迟到的 interrupt 不能产生额外消息");
        assert_eq!(messages[1].role, "assistant");

        let (close_tx, close_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::Close,
                reply_to: close_tx,
            })
            .await
            .expect("关闭应能投递");
        close_reply
            .await
            .expect("应返回关闭结果")
            .expect("关闭应成功");
        actor_task.await.expect("Actor 不应 panic");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn approval_request_keeps_actor_responsive_until_resolve() {
        let path = test_path("approval");
        let session_id = SessionId::new("actor-approval");
        let store = prepare_store(&path, &session_id);
        let (sender, receiver) = mpsc::channel(8);
        let (events, mut event_receiver) = broadcast::channel(16);
        let actor = SessionActor::new(session_id.clone(), store, receiver, sender.clone(), events);
        let actor_task = tokio::spawn(actor.run());

        let (submit_tx, submit_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::SubmitPrompt {
                    request_id: RequestId::new(),
                    input: UserInput::new("审批测试").expect("输入有效"),
                },
                reply_to: submit_tx,
            })
            .await
            .expect("提交应能投递");
        let turn_id = match submit_reply.await.expect("应返回结果").expect("提交应成功") {
            CommandReply::Accepted { turn_id } => turn_id,
            _ => panic!("应返回 Accepted"),
        };
        let _ = event_receiver.recv().await;
        let _ = event_receiver.recv().await;

        let request = ApprovalRequest {
            approval_id: sagent_types::ApprovalId::new(),
            session_id: session_id.clone(),
            turn_id,
            tool_call_id: ToolCallId::new(),
            tool_name: "terminal".into(),
            summary: "该命令需要审批".into(),
            policy_key: "terminal:recursive_delete".into(),
            expires_at: "2026-09-05T00:00:00Z".into(),
        };
        let approval_id = request.approval_id;
        sender
            .send(ActorInput::ApprovalRequired { turn_id, request })
            .await
            .expect("审批请求应能投递");
        let requested = event_receiver.recv().await.expect("应收到审批事件");
        assert!(matches!(
            requested.kind,
            RuntimeEventKind::ApprovalRequested { approval_id: id, .. } if id == approval_id
        ));

        let (resolve_tx, resolve_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::ResolveApproval {
                    approval_id,
                    decision: sagent_agent::ApprovalDecision::Once,
                },
                reply_to: resolve_tx,
            })
            .await
            .expect("审批响应应能投递");
        assert!(matches!(
            resolve_reply
                .await
                .expect("应返回审批结果")
                .expect("审批应被接收"),
            CommandReply::ApprovalAccepted
        ));
        let resolved = event_receiver.recv().await.expect("应收到审批完成事件");
        assert!(matches!(
            resolved.kind,
            RuntimeEventKind::ApprovalResolved { approval_id: id, decision: sagent_agent::ApprovalDecision::Once }
                if id == approval_id
        ));
        let persisted = Store::open_readonly(&path)
            .expect("应能读取审批事件")
            .events_since(&EventQuery {
                session_id: session_id.clone(),
                after_sequence: EventSequence::new(0).expect("序号有效"),
                limit: 200,
            })
            .expect("审批事件查询应成功");
        assert!(
            persisted
                .iter()
                .any(|event| event.event_type == "approval.requested")
        );
        assert!(
            persisted
                .iter()
                .any(|event| event.event_type == "approval.resolved")
        );

        let (close_tx, close_reply) = oneshot::channel();
        sender
            .send(ActorInput::Command {
                command: SessionCommand::Close,
                reply_to: close_tx,
            })
            .await
            .expect("关闭应能投递");
        close_reply
            .await
            .expect("应返回关闭结果")
            .expect("关闭应成功");
        actor_task.await.expect("Actor 不应 panic");
        let _ = fs::remove_file(path);
    }
}
