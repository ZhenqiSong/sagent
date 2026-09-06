//! 受 Runtime 监管的工具执行 worker。
//!
//! ToolWorker 只负责调用 `sagent-tools` 中的具体服务，不写 Store，也不发布
//! RuntimeEvent。Provider 的原始 `call_id` 在结果中保留；底层工具需要的本地
//! `ToolCallId` 仅用于进程跟踪和结果构造，不能暴露为上游调用 ID。

use sagent_tools::{
    ReadFileLimits, ReadFileRequest, ReadFileService, TerminalExecutor, TerminalLimits,
    TerminalRequest, ToolResult, WorkspaceRoot,
};
use sagent_types::ToolCallId;
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::tool_dispatch::ToolDispatchPlan;

/// 工具执行后回传给 Actor 的模型无关结果。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ToolExecutionResult {
    pub call_id: String,
    pub name: String,
    pub ok: bool,
    pub content: String,
    pub truncated: bool,
    pub exit_code: Option<i32>,
    pub error_kind: Option<String>,
}

/// 参数不能反序列化为工具请求时的稳定错误。
#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum ToolWorkerError {
    #[error("工具参数无效：{tool_name}: {reason}")]
    InvalidArguments { tool_name: String, reason: String },
    #[error("工具执行已取消")]
    Cancelled,
}

/// Runtime 持有的具体工具服务集合。
#[derive(Debug, Clone)]
pub struct ToolWorker {
    read_file: ReadFileService,
    terminal: TerminalExecutor,
}

impl ToolWorker {
    pub fn new(
        workspace: WorkspaceRoot,
        read_file_limits: ReadFileLimits,
        terminal_limits: TerminalLimits,
    ) -> Self {
        Self {
            read_file: ReadFileService::new(workspace.clone(), read_file_limits),
            terminal: TerminalExecutor::new(workspace, terminal_limits),
        }
    }

    pub fn read_file(&self) -> &ReadFileService {
        &self.read_file
    }

    pub fn terminal(&self) -> &TerminalExecutor {
        &self.terminal
    }

    /// 顺序执行一批调用；第一版不并行，以保证 transcript 顺序稳定。
    pub async fn execute_batch(
        &self,
        plans: impl IntoIterator<Item = ToolDispatchPlan>,
        cancellation: CancellationToken,
    ) -> Vec<ToolExecutionResult> {
        let mut results = Vec::new();
        for plan in plans {
            if cancellation.is_cancelled() {
                break;
            }
            results.push(self.execute(plan, cancellation.child_token()).await);
        }
        results
    }

    /// 执行单个 registry 计划。
    pub async fn execute(
        &self,
        plan: ToolDispatchPlan,
        cancellation: CancellationToken,
    ) -> ToolExecutionResult {
        let call_id = plan.call.call_id.clone();
        let name = plan.call.name.clone();
        let result = match self.execute_inner(&plan, cancellation).await {
            Ok(result) => result,
            Err(ToolWorkerError::InvalidArguments { tool_name, reason }) => ToolResult::failure(
                ToolCallId::new(),
                tool_name,
                "invalid_arguments",
                reason,
                plan.output_limit,
                None,
            ),
            Err(ToolWorkerError::Cancelled) => ToolResult::failure(
                ToolCallId::new(),
                name.clone(),
                "cancelled",
                "工具执行已取消",
                plan.output_limit,
                None,
            ),
        };
        ToolExecutionResult {
            call_id,
            name: result.name,
            ok: result.ok,
            content: result.content,
            truncated: result.truncated,
            exit_code: result.exit_code,
            error_kind: result.error_kind,
        }
    }

    async fn execute_inner(
        &self,
        plan: &ToolDispatchPlan,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolWorkerError> {
        if cancellation.is_cancelled() {
            return Err(ToolWorkerError::Cancelled);
        }
        let local_call_id = ToolCallId::new();
        match plan.call.name.as_str() {
            "read_file" => {
                let request = parse_request::<ReadFileRequest>(&plan.call.arguments, "read_file")?;
                Ok(self
                    .read_file
                    .read(local_call_id, request, cancellation)
                    .await)
            }
            "terminal" => {
                let request = parse_request::<TerminalRequest>(&plan.call.arguments, "terminal")?;
                Ok(self
                    .terminal
                    .execute_authorized(local_call_id, request, cancellation)
                    .await)
            }
            _ => Err(ToolWorkerError::InvalidArguments {
                tool_name: plan.call.name.clone(),
                reason: "工具没有对应的执行器".into(),
            }),
        }
    }
}

fn parse_request<T: serde::de::DeserializeOwned>(
    arguments: &Value,
    tool_name: &str,
) -> Result<T, ToolWorkerError> {
    serde_json::from_value(arguments.clone()).map_err(|error| ToolWorkerError::InvalidArguments {
        tool_name: tool_name.into(),
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::ToolWorker;
    use crate::{ToolCall, ToolDispatcher};
    use sagent_tools::{
        ReadFileLimits, TerminalLimits, ToolDefinition, ToolPermission, ToolRegistry, WorkspaceRoot,
    };
    use serde_json::json;
    use std::fs;
    use tokio_util::sync::CancellationToken;

    fn worker() -> (ToolWorker, ToolDispatcher, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "sagent-runtime-tool-worker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("notes.txt"), "第一行\n第二行").unwrap();
        let workspace = WorkspaceRoot::new(&root).unwrap();
        let tool_worker = ToolWorker::new(
            workspace,
            ReadFileLimits::default(),
            TerminalLimits::default(),
        );
        let mut registry = ToolRegistry::new();
        registry
            .register(
                ToolDefinition::new(
                    "read_file",
                    "读取文件",
                    json!({"type": "object"}),
                    ToolPermission::ReadOnly,
                    30_000,
                    32_768,
                )
                .unwrap(),
            )
            .unwrap();
        registry
            .register(
                ToolDefinition::new(
                    "terminal",
                    "执行命令",
                    json!({"type": "object"}),
                    ToolPermission::ReadOnly,
                    30_000,
                    32_768,
                )
                .unwrap(),
            )
            .unwrap();
        (tool_worker, ToolDispatcher::new(registry), root)
    }

    #[tokio::test]
    async fn executes_read_file_and_preserves_provider_call_id() {
        let (worker, dispatcher, root) = worker();
        let plans = dispatcher
            .plan(vec![ToolCall {
                call_id: "call_provider_1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "notes.txt", "offset": 1, "limit": 1}),
            }])
            .unwrap();
        let results = worker.execute_batch(plans, CancellationToken::new()).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].call_id, "call_provider_1");
        assert!(results[0].ok);
        assert_eq!(results[0].content, "1|第一行");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn cancellation_stops_a_tool_batch_before_execution() {
        let (worker, dispatcher, root) = worker();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let plans = dispatcher
            .plan(vec![ToolCall {
                call_id: "call_cancelled".into(),
                name: "read_file".into(),
                arguments: json!({"path": "notes.txt"}),
            }])
            .unwrap();
        assert!(worker.execute_batch(plans, cancellation).await.is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
