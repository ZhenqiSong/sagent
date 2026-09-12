//! 受 Runtime 监管的工具执行 worker。
//!
//! ToolWorker 只负责调用 `sagent-tools` 中的具体服务，不写 Store，也不发布
//! RuntimeEvent。Provider 的原始 `call_id` 在结果中保留；底层工具需要的本地
//! `ToolCallId` 仅用于进程跟踪和结果构造，不能暴露为上游调用 ID。

use sagent_tools::{
    ReadFileLimits, ReadFileRequest, ReadFileService, SessionSearchRequest, SessionSearchService,
    TerminalExecutor, TerminalLimits, TerminalRequest, ToolResult, WorkspaceRoot, WriteFileLimits,
    WriteFileRequest, WriteFileService,
};
use sagent_types::ToolCallId;
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::tool_dispatch::ToolDispatchPlan;

/// 工具执行后回传给 Actor 的模型无关结果。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ToolExecutionResult {
    /// Provider 原始调用 ID，用于和 assistant tool call 配对。
    pub call_id: String,
    /// 工具名称。
    pub name: String,
    /// 工具是否成功完成。
    pub ok: bool,
    /// 受输出限制后的正文。
    pub content: String,
    /// 是否丢弃了超出上限的输出。
    pub truncated: bool,
    /// 进程退出码；非进程工具通常为空。
    pub exit_code: Option<i32>,
    /// 稳定的机器可读失败分类。
    pub error_kind: Option<String>,
}

/// 参数不能反序列化为工具请求时的稳定错误。
#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum ToolWorkerError {
    #[error("工具参数无效：{tool_name}: {reason}")]
    InvalidArguments { tool_name: String, reason: String },
    #[error("工具未配置：{tool_name}")]
    Unavailable { tool_name: String },
    #[error("工具执行已取消")]
    Cancelled,
}

/// Runtime 持有的具体工具服务集合。
#[derive(Debug, Clone)]
pub struct ToolWorker {
    read_file: ReadFileService,
    write_file: WriteFileService,
    session_search: Option<SessionSearchService>,
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
            // 写入限制不是 runtime 可配置项；它使用保守默认值，避免为尚未稳定的工具
            // 新增 Profile 配置面。后续需要配置时应与 ToolDefinition 一起版本化。
            write_file: WriteFileService::new(workspace.clone(), WriteFileLimits::default()),
            session_search: None,
            terminal: TerminalExecutor::new(workspace, terminal_limits),
        }
    }

    pub fn read_file(&self) -> &ReadFileService {
        &self.read_file
    }

    pub fn terminal(&self) -> &TerminalExecutor {
        &self.terminal
    }

    /// 返回受 workspace 边界和原子提交约束的写入服务。
    pub fn write_file(&self) -> &WriteFileService {
        &self.write_file
    }

    /// 绑定当前 Profile 的只读搜索数据库；未绑定时 session_search fail-closed。
    pub fn with_session_search(mut self, state_db: impl Into<std::path::PathBuf>) -> Self {
        self.session_search = Some(SessionSearchService::new(state_db, Default::default()));
        self
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
            Err(ToolWorkerError::Unavailable { tool_name }) => ToolResult::failure(
                ToolCallId::new(),
                tool_name,
                "tool_unavailable",
                "当前 Runtime 未配置该工具",
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
            "write_file" => {
                let request =
                    parse_request::<WriteFileRequest>(&plan.call.arguments, "write_file")?;
                Ok(self
                    .write_file
                    .write(local_call_id, request, cancellation)
                    .await)
            }
            "session_search" => {
                let request =
                    parse_request::<SessionSearchRequest>(&plan.call.arguments, "session_search")?;
                let Some(service) = self.session_search.as_ref() else {
                    return Err(ToolWorkerError::Unavailable {
                        tool_name: "session_search".into(),
                    });
                };
                Ok(service.search(local_call_id, request, cancellation).await)
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
    use sagent_store::{NewMessage, NewSession, Store};
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
                    "session_search",
                    "搜索当前 Profile 的会话消息",
                    json!({
                        "type": "object",
                        "required": ["query"],
                        "properties": {
                            "query": {"type": "string"},
                            "limit": {"type": "integer", "minimum": 1},
                            "session_id": {"type": "string"}
                        },
                        "additionalProperties": false
                    }),
                    ToolPermission::ReadOnly,
                    30_000,
                    16_384,
                )
                .unwrap(),
            )
            .unwrap();
        registry
            .register(
                ToolDefinition::new(
                    "write_file",
                    "在工作区内原子写入文本文件",
                    json!({"type": "object", "required": ["path", "content"]}),
                    ToolPermission::ApprovalRequired,
                    30_000,
                    4_096,
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

    #[tokio::test]
    async fn executes_write_file_only_after_the_dispatcher_has_planned_it() {
        let (worker, dispatcher, root) = worker();
        let plans = dispatcher
            .plan(vec![ToolCall {
                call_id: "call_write_1".into(),
                name: "write_file".into(),
                arguments: json!({"path": "created.txt", "content": "受控写入"}),
            }])
            .expect("已注册 write_file 应能生成计划");
        let results = worker.execute_batch(plans, CancellationToken::new()).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].ok);
        assert_eq!(results[0].call_id, "call_write_1");
        assert_eq!(
            fs::read_to_string(root.join("created.txt")).unwrap(),
            "受控写入"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn executes_session_search_against_the_bound_profile_database() {
        let (worker, dispatcher, root) = worker();
        let database = root.join("state.db");
        let session_id = sagent_types::SessionId::new("search-worker-session");
        let mut store = Store::open_readwrite(&database).expect("应能创建搜索数据库");
        store
            .create_session(&NewSession {
                id: session_id.clone(),
                source: Some("test".into()),
                model: None,
                title: None,
                started_at: "2026-09-10T00:00:00Z".into(),
            })
            .expect("应能创建搜索会话");
        store
            .append_message(&NewMessage::new(
                session_id.clone(),
                "user",
                "Rust 搜索内容 🚀",
                "2026-09-10T00:00:01Z",
            ))
            .expect("应能写入搜索消息");
        drop(store);

        let worker = worker.with_session_search(database);
        let plans = dispatcher
            .plan(vec![ToolCall {
                call_id: "call_search_1".into(),
                name: "session_search".into(),
                arguments: json!({"query": "Rust", "session_id": "search-worker-session"}),
            }])
            .expect("已注册 session_search 应能生成计划");
        let results = worker.execute_batch(plans, CancellationToken::new()).await;
        assert!(results[0].ok);
        assert!(results[0].content.contains("search-worker-session"));
        let _ = fs::remove_dir_all(root);
    }
}
