//! SessionActor 的 generation、工具 schema 与模型上下文。
//!
//! 本模块负责把持久化的 generation 与当前模型上下文保持一致，并为工具回环重建下一轮
//! Provider 请求；不负责 prompt 首轮提交或 Turn 终态收口。

use sagent_agent::{PromptMessage, PromptRole, PromptSnapshot, PromptToolCall, TurnState};
use sagent_store::{MessageQuery, NewGeneration};
use sagent_types::TurnId;

use super::{ActorModelRuntime, SessionActor};
use crate::{RuntimeError, input::ActorInput, provider_worker::spawn_provider_worker};

const EMPTY_TOOL_SCHEMA_HASH: &str = "sha256:empty-tools";

impl SessionActor {
    pub(super) fn ensure_generation(
        &mut self,
        system_hash: &str,
        tool_schema_hash: &str,
    ) -> Result<(), RuntimeError> {
        match self
            .context
            .query_storage
            .get_generation(&self.context.session_id, self.lifecycle.generation)
            .map_err(|error| RuntimeError::Persistence(error.to_string()))?
        {
            Some(generation)
                if generation.system_hash == system_hash
                    && generation.tool_schema_hash == tool_schema_hash => {}
            Some(_) => return Err(RuntimeError::RequiresTransition),
            None => self
                .context
                .session_storage
                .create_generation(&NewGeneration {
                    session_id: self.context.session_id.clone(),
                    generation: self.lifecycle.generation,
                    system_hash: system_hash.to_owned(),
                    tool_schema_hash: tool_schema_hash.to_owned(),
                    model_id: self.model_runtime.model().to_owned(),
                    profile_revision: self.model_runtime.profile_revision().to_owned(),
                    created_at: (self.context.clock)(),
                })
                .map_err(|error| RuntimeError::Persistence(error.to_string()))?,
        }
        Ok(())
    }

    /// 返回本 generation 对模型可见的稳定工具 schema 和 fingerprint。
    pub(super) fn tool_schema(&self) -> Result<(String, Vec<serde_json::Value>), RuntimeError> {
        let Some(dispatcher) = self.tool_runtime.dispatcher() else {
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
            .context
            .query_storage
            .get_messages_for_model(&self.context.session_id, &MessageQuery::default())
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
        PromptSnapshot::new(self.context.session_id.clone(), turn_id, &system, messages)
            .map_err(|error| RuntimeError::InvalidLifecycle(error.to_string()))
    }

    /// 用已持久化的 user、assistant(tool_calls) 和 tool messages 启动下一轮模型调用。
    pub(super) fn start_next_provider_round(
        &mut self,
        turn_id: TurnId,
    ) -> Result<(), RuntimeError> {
        let (provider, model) = match &self.model_runtime {
            ActorModelRuntime::Provider {
                provider, model, ..
            } => (provider.clone(), model.clone()),
            ActorModelRuntime::Unconfigured { .. } => {
                return Err(RuntimeError::InvalidLifecycle(
                    "Provider 未配置，不能继续工具回环".into(),
                ));
            }
            #[cfg(test)]
            ActorModelRuntime::Test { .. } => {
                return Err(RuntimeError::InvalidLifecycle(
                    "Provider 未配置，不能继续工具回环".into(),
                ));
            }
        };
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
            model,
            request_id,
            tools,
            self.channels.command_tx.clone(),
            cancellation,
        );
        let worker_abort = worker.abort_handle();
        let sender = self.channels.command_tx.clone();
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
}
