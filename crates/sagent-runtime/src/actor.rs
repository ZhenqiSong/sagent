//! SessionActor 的最小 submit 处理循环。

use std::sync::Arc;
use std::time::Duration;

use sagent_agent::{
    PromptMessage, PromptRole, PromptSnapshot, PromptToolCall, RequestId, SessionCommand,
    SystemPromptParts, TurnState, UserInput,
};
use sagent_provider::ModelProvider;
use sagent_store::{MessageQuery, NewDaemonEvent, NewGeneration, NewMessage, StartTurn, Store};
use sagent_types::{SessionId, TurnId};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    RuntimeError,
    active_turn::ActiveTurn,
    approval::ApprovalManager,
    event::{RuntimeEvent, RuntimeEventKind},
    input::{ActorInput, CommandReply},
    provider_worker::spawn_provider_worker,
    tool_dispatch::ToolDispatcher,
    tool_worker::{ToolExecutionResult, ToolWorker},
};

const DEFAULT_MODEL_ID: &str = "unconfigured";
const DEFAULT_PROFILE_REVISION: &str = "runtime-v1";
const EMPTY_TOOL_SCHEMA_HASH: &str = "sha256:empty-tools";
const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
const DEFAULT_MAX_TOOL_ROUNDS: u32 = 8;

// 工具/审批会改变同一 ActiveTurn，故作为 actor 的私有子模块共享其私有状态；不能提升为
// 独立 service，否则 worker 与 mailbox 对状态转换的唯一写入者约束会被打破。
#[path = "actor/tools.rs"]
mod tools;

// Provider 和 Tool worker 的结果都经 mailbox 回到这里；单独归类可让主 Actor 文件
// 专注命令入口和终态收口，而不改变结果处理的串行化边界。
#[path = "actor/worker.rs"]
mod worker;

#[cfg(test)]
#[path = "actor/tests.rs"]
mod tests;

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
                // 正常 Provider 结果已经通过同一个 worker task 先投递为 WorkerEvent；
                // 监控 task 使用另一 sender 时无法保证跨 sender 顺序，若把 Ok 退出也
                // 投递会与 ToolCalls/FinalText 竞争并提前失败 Turn。只有 JoinError
                //（例如 worker panic）才需要补一条失败事实。
                if let Err(error) = worker.await {
                    let _ = sender
                        .send(ActorInput::WorkerExited {
                            turn_id,
                            result: Err(crate::input::WorkerFailure(error.to_string())),
                        })
                        .await;
                }
            })
        });
        self.active = Some(ActiveTurn {
            turn_id,
            request_id,
            generation: self.generation,
            tool_rounds: 0,
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
            // 正常的 ToolCalls/FinalText 已先由 Provider worker 发出；这里只处理
            // JoinError，避免独立 monitor sender 把正常退出排到业务事件之前。
            if let Err(error) = worker.await {
                let _ = sender
                    .send(ActorInput::WorkerExited {
                        turn_id,
                        result: Err(crate::input::WorkerFailure(error.to_string())),
                    })
                    .await;
            }
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

    async fn interrupt_active(&mut self, reason: &str) -> Result<CommandReply, RuntimeError> {
        let turn_id = self
            .active
            .as_ref()
            .map(|active| active.turn_id)
            .ok_or(RuntimeError::NoActiveTurn)?;
        self.interrupt_active_for_turn(turn_id, reason).await
    }

    /// 将指定的活跃 Turn 收口为 interrupted，并保证终态只在持久化成功后对外可见。
    ///
    /// 此函数先把 `active` 暂时移出 Actor，阻止 worker、审批回调与其它命令在收口期间
    /// 重复处理同一 Turn；随后依次取消执行链、停止受监管任务、原子写入 Store，最后才
    /// 发布 `TurnInterrupted`。若持久化失败，必须恢复 `active`，使调用方得到错误而不把
    /// 内存状态伪装成已结束；成功时不再放回它，Actor 因而回到可接受下一轮提交的空闲状态。
    ///
    /// 不会写入虚构的 assistant 消息。`reason` 会持久化为该 Turn 的中断原因，并用于区分
    /// 用户主动取消与会话关闭等终止来源。
    async fn interrupt_active_for_turn(
        &mut self,
        turn_id: TurnId,
        reason: &str,
    ) -> Result<CommandReply, RuntimeError> {
        // 先从 Actor 状态中摘下 active：收口期间迟到的 worker/审批事件将无法再匹配
        // 该 Turn，从而不会与中断路径竞争并重复写入终态。
        let Some(mut active) = self.take_active(turn_id) else {
            return Err(RuntimeError::NoActiveTurn);
        };
        if active.terminal {
            // 理论上已终态的 Turn 不应仍在 active 中；保留原状态以免错误路径改变它。
            self.active = Some(active);
            return Err(RuntimeError::NoActiveTurn);
        }
        // 先传播取消并撤销审批，再等待/终止受监管任务，确保没有后台工作在持久化
        // interrupted 后继续产出结果或启动外部进程。
        active.cancellation.cancel();
        self.approvals.cancel_for_turn(&turn_id);
        Self::stop_worker(&mut active).await;
        let timestamp = (self.clock)();
        if let Err(error) = self.store.interrupt_turn(&turn_id, reason, &timestamp) {
            // 数据库仍是终态事实的唯一来源；写入失败时恢复 active，让调用方显式处理
            // 持久化错误，而不是把内存会话静默变成空闲。
            self.active = Some(active);
            return Err(RuntimeError::Persistence(error.to_string()));
        }
        // 只有 Store 成功提交后才发布终态事件。成功路径不放回 active，释放该 Actor
        // 以接受后续 Turn。
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
