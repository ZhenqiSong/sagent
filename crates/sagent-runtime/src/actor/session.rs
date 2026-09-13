//! SessionActor 的生命周期入口与 mailbox 分发。
//!
//! 该模块只负责装配一个 Actor、消费 mailbox、路由命令/worker 事实以及发布通用事件。
//! prompt、generation、工具和 Turn 终态逻辑分别位于同级主题模块中。

use std::sync::Arc;
use std::time::Duration;

use sagent_agent::SessionCommand;
use sagent_provider::ModelProvider;
use sagent_store::{NewDaemonEvent, StorageDependencies};
use sagent_types::{SessionId, TurnId};
use tokio::sync::{broadcast, mpsc};

#[cfg(test)]
use super::WorkerFactory;
use super::{
    ActorChannels, ActorLifecycle, ActorModelRuntime, ActorPolicy, ActorSessionContext,
    ActorToolRuntime,
};
use crate::{
    RuntimeError,
    active_turn::ActiveTurn,
    approval::ApprovalManager,
    event::{RuntimeEvent, RuntimeEventKind},
    input::{ActorInput, CommandReply},
    tool_dispatch::ToolDispatcher,
    tool_worker::{ToolExecutionResult, ToolWorker},
};

const DEFAULT_MODEL_ID: &str = "unconfigured";
const DEFAULT_PROFILE_REVISION: &str = "runtime-v1";
const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
const DEFAULT_MAX_TOOL_ROUNDS: u32 = 8;

/// 一个 Session 的唯一写入者。
pub(crate) struct SessionActor {
    pub(super) context: ActorSessionContext,
    pub(super) channels: ActorChannels,
    pub(super) active: Option<ActiveTurn>,
    pub(super) lifecycle: ActorLifecycle,
    pub(super) model_runtime: ActorModelRuntime,
    pub(super) tool_runtime: ActorToolRuntime,
    pub(super) policy: ActorPolicy,
}

impl SessionActor {
    /// 创建不启动 worker 的 Actor；单测可直接驱动提交边界和 mailbox 状态机。
    pub(crate) fn new(
        session_id: SessionId,
        storage: impl Into<StorageDependencies>,
        command_rx: mpsc::Receiver<ActorInput>,
        command_tx: mpsc::Sender<ActorInput>,
        event_tx: broadcast::Sender<RuntimeEvent>,
    ) -> Self {
        let (session_storage, query_storage, _search_storage) = storage.into().into_parts();
        let startup_recovery = crate::recovery::plan_recovery(query_storage.as_ref(), &session_id);
        Self {
            context: ActorSessionContext {
                session_id,
                session_storage,
                query_storage,
                clock: utc_now,
            },
            channels: ActorChannels {
                command_rx,
                command_tx,
                event_tx,
            },
            active: None,
            lifecycle: ActorLifecycle {
                generation: 0,
                startup_recovery,
            },
            model_runtime: ActorModelRuntime::Unconfigured {
                model: DEFAULT_MODEL_ID.to_owned(),
                profile_revision: DEFAULT_PROFILE_REVISION.to_owned(),
            },
            tool_runtime: ActorToolRuntime::Disabled,
            policy: ActorPolicy {
                approvals: ApprovalManager::new(DEFAULT_APPROVAL_TIMEOUT),
                interactive_approval: true,
                max_tool_rounds: DEFAULT_MAX_TOOL_ROUNDS,
            },
        }
    }

    /// 为测试或后续 Provider 注入受监管的 worker 工厂和时钟。
    #[cfg(test)]
    pub(crate) fn with_worker_factory(
        mut self,
        worker_factory: WorkerFactory,
        clock: fn() -> String,
    ) -> Self {
        self.model_runtime = ActorModelRuntime::Test {
            worker_factory,
            model: DEFAULT_MODEL_ID.to_owned(),
            profile_revision: DEFAULT_PROFILE_REVISION.to_owned(),
        };
        self.context.clock = clock;
        self
    }

    /// 覆盖审批等待时长；生产环境由 Runtime 配置注入，测试可使用很短的超时。
    pub(crate) fn with_approval_timeout(mut self, timeout: Duration) -> Self {
        self.policy.approvals = ApprovalManager::new(timeout);
        self
    }

    /// 注入真实 Provider；Provider worker 仍由 Actor 监管，不能直接写 Store。
    pub(crate) fn with_provider(
        mut self,
        provider: Arc<dyn ModelProvider>,
        model: impl Into<String>,
        profile_revision: impl Into<String>,
    ) -> Self {
        self.model_runtime = ActorModelRuntime::Provider {
            provider,
            model: model.into(),
            profile_revision: profile_revision.into(),
        };
        self
    }

    /// 成对注入工具 registry 与执行器；工具 worker 不能直接写 Store。
    pub(crate) fn with_tools(mut self, dispatcher: ToolDispatcher, worker: ToolWorker) -> Self {
        self.tool_runtime = ActorToolRuntime::Enabled {
            dispatcher,
            worker: Arc::new(worker),
        };
        self
    }

    /// 限制单个 Turn 的工具回环次数，避免 Provider 重复发起同一工具调用。
    pub(crate) fn with_max_tool_rounds(mut self, limit: u32) -> Self {
        self.policy.max_tool_rounds = limit.max(1);
        self
    }

    /// 按顺序消费 mailbox；Store 只在这个循环所属的 Actor 中被写入。
    pub(crate) async fn run(mut self) {
        self.apply_startup_recovery();
        while let Some(input) = self.channels.command_rx.recv().await {
            if self.handle_input(input).await {
                break;
            }
        }
    }

    /// 将进程重启前遗留的 running Turn 安全收口。任何未完成工具都只会获得描述
    /// “未执行/结果未知”的持久化结果，绝不在恢复阶段重新启动工具或 Provider。
    fn apply_startup_recovery(&mut self) {
        let recovery = std::mem::replace(&mut self.lifecycle.startup_recovery, Ok(None));
        let plan = match recovery {
            Ok(Some(plan)) => plan,
            Ok(None) => return,
            Err(reason) => {
                self.publish(RuntimeEvent {
                    session_id: self.context.session_id.clone(),
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
                    session_id: self.context.session_id.clone(),
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
        if let Err(error) = self.context.session_storage.fail_turn(
            &plan.turn_id,
            "runtime_restarted",
            &reason,
            &(self.context.clock)(),
        ) {
            self.publish(RuntimeEvent {
                session_id: self.context.session_id.clone(),
                turn_id: Some(plan.turn_id),
                request_id: None,
                kind: RuntimeEventKind::TurnFailed {
                    reason: format!("写入重启终态失败：{error}"),
                },
            });
            return;
        }
        self.publish(RuntimeEvent {
            session_id: self.context.session_id.clone(),
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
                self.policy.interactive_approval = client.interactive_approval;
                Ok(CommandReply::Resumed)
            }
        }
    }

    /// 向订阅者发布已经由 Actor 串行确认的运行时事件。
    pub(super) fn publish(&self, event: RuntimeEvent) {
        let _ = self.channels.event_tx.send(event);
    }

    /// 持久化审批事实；调用方负责在成功后发布对应的运行时事件。
    pub(super) fn persist_approval_event(
        &mut self,
        turn_id: TurnId,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<(), String> {
        self.context
            .session_storage
            .append_event(&NewDaemonEvent {
                session_id: self.context.session_id.clone(),
                turn_id: Some(turn_id),
                event_type: event_type.to_owned(),
                payload,
                created_at: (self.context.clock)(),
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// 生成持久化事件所需的 UTC 秒级时间戳。
pub(crate) fn utc_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    format!("{seconds:020}")
}
