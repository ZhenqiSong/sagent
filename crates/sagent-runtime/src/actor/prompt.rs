//! SessionActor 的 prompt 提交与模型 worker 启动。
//!
//! 本模块只负责把一次 prompt 转换为不可变提交计划、持久化 user message，并在成功后
//! 启动受监管 worker；generation 校验和终态收口仍由其它主题模块负责。

use sagent_agent::{
    PromptMessage, PromptRole, PromptSnapshot, RequestId, SystemPromptParts, TurnState, UserInput,
};
use sagent_store::{NewMessage, StartTurn};
use sagent_types::{MessageId, TurnId};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;

use super::{ActorModelRuntime, SessionActor};
use crate::{
    RuntimeError,
    active_turn::ActiveTurn,
    event::{RuntimeEvent, RuntimeEventKind},
    input::ActorInput,
    provider_worker::spawn_provider_worker,
};

/// `submit_prompt` 在启动副作用前准备好的不可变输入快照。
///
/// 先计算 prompt、工具 schema 和 generation，再写入 Store，确保后续 worker 只能
/// 使用本次 Turn 已审计的内容，不会读取提交过程中的可变配置。
struct PromptSubmissionPlan {
    turn_id: TurnId,
    request_id: RequestId,
    user_content: String,
    system: SystemPromptParts,
    snapshot: PromptSnapshot,
    tools: Vec<serde_json::Value>,
}

/// 已启动但尚未安装到 `ActiveTurn` 的 worker 监控句柄。
struct PromptWorker {
    monitor: JoinHandle<()>,
    abort: AbortHandle,
}

impl SessionActor {
    /// 按固定顺序提交 prompt：校验、准备、持久化、启动 worker、安装状态、发布事件。
    pub(super) fn submit_prompt(
        &mut self,
        request_id: RequestId,
        input: UserInput,
    ) -> Result<crate::input::CommandReply, RuntimeError> {
        self.validate_prompt_submission()?;
        let plan = self.prepare_prompt_submission(request_id, input)?;
        let persisted_message_id = self.persist_prompt_start(&plan)?;
        let cancellation = CancellationToken::new();
        let worker = self.start_prompt_worker(&plan, &cancellation);
        self.install_active_turn(&plan, cancellation, worker);
        self.publish_prompt_events(&plan, persisted_message_id);
        Ok(crate::input::CommandReply::Accepted {
            turn_id: plan.turn_id,
        })
    }

    /// 提交前只检查当前 Actor 的状态； Busy 必须在任何 Store/worker 副作用前返回。
    fn validate_prompt_submission(&self) -> Result<(), RuntimeError> {
        if self.active.is_some() {
            return Err(RuntimeError::Busy {
                session_id: self.context.session_id.clone(),
            });
        }
        Ok(())
    }

    /// 构造本次 Turn 的 prompt、工具 schema 和 generation 计划，不产生外部副作用。
    fn prepare_prompt_submission(
        &mut self,
        request_id: RequestId,
        input: UserInput,
    ) -> Result<PromptSubmissionPlan, RuntimeError> {
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
        let snapshot =
            PromptSnapshot::new(self.context.session_id.clone(), turn_id, &system, messages)
                .map_err(|error| RuntimeError::InvalidLifecycle(error.to_string()))?;
        let system_hash = snapshot.system_prompt_hash.clone();
        let (tool_schema_hash, tools) = self.tool_schema()?;
        self.ensure_generation(&system_hash, &tool_schema_hash)?;

        Ok(PromptSubmissionPlan {
            turn_id,
            request_id,
            user_content: input.as_str().to_owned(),
            system,
            snapshot,
            tools,
        })
    }

    /// 在单个 Store 原子边界中记录 Turn 与 user message，成功前不启动 worker。
    fn persist_prompt_start(
        &mut self,
        plan: &PromptSubmissionPlan,
    ) -> Result<MessageId, RuntimeError> {
        let timestamp = (self.context.clock)();
        let message = NewMessage::new(
            self.context.session_id.clone(),
            "user",
            &plan.user_content,
            timestamp.clone(),
        );
        self.context
            .store
            .begin_turn(
                &StartTurn {
                    turn_id: plan.turn_id,
                    session_id: self.context.session_id.clone(),
                    generation: self.lifecycle.generation,
                    started_at: timestamp,
                },
                &message,
            )
            .map_err(|error| RuntimeError::Persistence(error.to_string()))
    }

    /// 按已审计的 plan 启动 Provider 或测试 worker，并把 JoinError 转回 Actor mailbox。
    fn start_prompt_worker(
        &self,
        plan: &PromptSubmissionPlan,
        cancellation: &CancellationToken,
    ) -> Option<PromptWorker> {
        let worker = match &self.model_runtime {
            ActorModelRuntime::Provider {
                provider, model, ..
            } => Some(spawn_provider_worker(
                provider.clone(),
                plan.snapshot.clone(),
                model.clone(),
                plan.request_id,
                plan.tools.clone(),
                self.channels.command_tx.clone(),
                cancellation.child_token(),
            )),
            #[cfg(test)]
            ActorModelRuntime::Test { worker_factory, .. } => Some(worker_factory(
                self.channels.command_tx.clone(),
                plan.turn_id,
                cancellation.child_token(),
            )),
            ActorModelRuntime::Unconfigured { .. } => None,
        };
        let worker = worker?;
        let abort = worker.abort_handle();
        let monitor = {
            let sender = self.channels.command_tx.clone();
            let turn_id = plan.turn_id;
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
        };
        Some(PromptWorker { monitor, abort })
    }

    /// 将 worker 和取消令牌安装为 Actor 的唯一 active state。
    fn install_active_turn(
        &mut self,
        plan: &PromptSubmissionPlan,
        cancellation: CancellationToken,
        worker: Option<PromptWorker>,
    ) {
        let (worker, worker_abort) = worker
            .map(|worker| (Some(worker.monitor), Some(worker.abort)))
            .unwrap_or((None, None));
        self.active = Some(ActiveTurn {
            turn_id: plan.turn_id,
            request_id: plan.request_id,
            generation: self.lifecycle.generation,
            tool_rounds: 0,
            system: plan.system.clone(),
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
    }

    /// 只有 Store 提交和 active state 安装成功后，才按既定顺序发布提交事件。
    fn publish_prompt_events(&self, plan: &PromptSubmissionPlan, persisted_message_id: MessageId) {
        self.publish(RuntimeEvent {
            session_id: self.context.session_id.clone(),
            turn_id: Some(plan.turn_id),
            request_id: Some(plan.request_id),
            kind: RuntimeEventKind::PromptAccepted,
        });
        self.publish(RuntimeEvent {
            session_id: self.context.session_id.clone(),
            turn_id: Some(plan.turn_id),
            request_id: Some(plan.request_id),
            kind: RuntimeEventKind::UserMessagePersisted {
                message_id: persisted_message_id,
            },
        });
    }
}
