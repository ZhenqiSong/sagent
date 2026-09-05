//! 安全的前台 terminal 执行器。

use std::time::Duration;

use sagent_types::ToolCallId;
use serde::Deserialize;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::command_policy::{CommandRisk, classify_command};
use crate::process::{
    BoundedOutput, ProcessSupervisor, TerminationReason, attach_process_tree_guard,
    configure_process_group, create_process_tree_guard, drain_output, sanitize_environment,
    shell_command, terminate_pid_tree, terminate_tree,
};
use crate::{ToolResult, WorkspaceError, WorkspaceRoot};

const TOOL_NAME: &str = "terminal";
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const DEFAULT_OUTPUT_LIMIT: usize = 32_768;
const MAX_OUTPUT_LIMIT: usize = 200_000;

#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
pub struct TerminalRequest {
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_output_limit")]
    pub output_limit: usize,
}

impl TerminalRequest {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            cwd: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            output_limit: DEFAULT_OUTPUT_LIMIT,
        }
    }
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

fn default_output_limit() -> usize {
    DEFAULT_OUTPUT_LIMIT
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TerminalLimits {
    pub default_timeout_ms: u64,
    pub max_timeout_ms: u64,
    pub default_output_limit: usize,
    pub max_output_limit: usize,
}

impl Default for TerminalLimits {
    fn default() -> Self {
        Self {
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
            max_timeout_ms: MAX_TIMEOUT_MS,
            default_output_limit: DEFAULT_OUTPUT_LIMIT,
            max_output_limit: MAX_OUTPUT_LIMIT,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TerminalExecutor {
    workspace: WorkspaceRoot,
    limits: TerminalLimits,
    supervisor: ProcessSupervisor,
}

impl TerminalExecutor {
    pub fn new(workspace: WorkspaceRoot, limits: TerminalLimits) -> Self {
        Self {
            workspace,
            limits,
            supervisor: ProcessSupervisor::new(),
        }
    }

    pub fn supervisor(&self) -> &ProcessSupervisor {
        &self.supervisor
    }

    pub async fn execute(
        &self,
        tool_call_id: ToolCallId,
        request: TerminalRequest,
        cancellation: CancellationToken,
    ) -> ToolResult {
        if request.command.trim().is_empty() {
            return failure(
                tool_call_id,
                "invalid_command",
                "命令不能为空",
                &self.limits,
                None,
            );
        }
        if request.timeout_ms == 0 || request.timeout_ms > self.limits.max_timeout_ms {
            return failure(
                tool_call_id,
                "invalid_timeout",
                "命令超出最大执行时间",
                &self.limits,
                None,
            );
        }
        if request.output_limit == 0 || request.output_limit > self.limits.max_output_limit {
            return failure(
                tool_call_id,
                "invalid_output_limit",
                "命令输出上限过大",
                &self.limits,
                None,
            );
        }
        if cancellation.is_cancelled() {
            return failure(
                tool_call_id,
                "cancelled",
                "命令执行已取消",
                &self.limits,
                None,
            );
        }

        let cwd = match self
            .workspace
            .resolve_directory(request.cwd.as_deref().unwrap_or("."))
        {
            Ok(cwd) => cwd,
            Err(error) => return self.workspace_failure(tool_call_id, error, &request),
        };
        match classify_command(&request.command) {
            CommandRisk::Safe => {}
            CommandRisk::RequireApproval { summary, .. } => {
                return failure(
                    tool_call_id,
                    "approval_required",
                    &summary,
                    &self.limits,
                    None,
                );
            }
            CommandRisk::Deny { reason } => {
                return failure(tool_call_id, "command_denied", &reason, &self.limits, None);
            }
        }

        let mut command = shell_command(&request.command);
        command.current_dir(cwd);
        command.env_clear();
        command.envs(sanitize_environment(std::env::vars()));
        configure_process_group(&mut command);
        // Windows Job Object 创建失败时保留 taskkill fallback；不能因为宿主本身
        // 已经处于另一个 Job Object 就让安全命令完全不可用。
        let process_guard = create_process_tree_guard().ok();
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                return failure(
                    tool_call_id,
                    "spawn_failed",
                    "无法启动 terminal 进程",
                    &self.limits,
                    None,
                );
            }
        };
        if let Some(guard) = process_guard.as_ref() {
            let _ = attach_process_tree_guard(guard, child.id().unwrap_or_default());
        }
        let process_id = child.id().unwrap_or_default();
        let id = tool_call_id.as_uuid().to_string();
        self.supervisor.register(id.clone());
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let timeout_duration = Duration::from_millis(request.timeout_ms);
        let output_future = drain_output(stdout, stderr, request.output_limit);
        tokio::pin!(output_future);
        let (status, terminal_error) = tokio::select! {
            result = child.wait() => (result.ok(), None),
            _ = sleep(timeout_duration) => {
                terminate_tree(&mut child, TerminationReason::Timeout).await;
                (None, Some(("timeout", "命令执行超时")))
            }
            _ = cancellation.cancelled() => {
                terminate_tree(&mut child, TerminationReason::Cancelled).await;
                (None, Some(("cancelled", "命令执行已取消")))
            }
        };
        let (out, err) = match tokio::time::timeout(Duration::from_secs(2), output_future).await {
            Ok(output) => output,
            Err(_) => {
                terminate_pid_tree(process_id).await;
                (
                    BoundedOutput {
                        content: String::new(),
                        truncated: true,
                    },
                    BoundedOutput {
                        content: String::new(),
                        truncated: true,
                    },
                )
            }
        };
        self.supervisor.unregister(&id);
        if let Some((kind, message)) = terminal_error {
            return failure_with_output(tool_call_id, kind, message, out, err, &request);
        }
        let exit_code = status.and_then(|status| status.code());
        let content = combine_output(out.content, err.content);
        let truncated = out.truncated || err.truncated;
        let mut result = if exit_code == Some(0) {
            ToolResult::success(
                tool_call_id,
                TOOL_NAME,
                content,
                request.output_limit,
                exit_code,
            )
        } else {
            ToolResult::failure(
                tool_call_id,
                TOOL_NAME,
                "non_zero_exit",
                content,
                request.output_limit,
                exit_code,
            )
        };
        result.truncated |= truncated;
        result
    }

    fn workspace_failure(
        &self,
        tool_call_id: ToolCallId,
        error: WorkspaceError,
        _request: &TerminalRequest,
    ) -> ToolResult {
        let (kind, message) = match error {
            WorkspaceError::PathDenied => ("path_denied", "cwd 不在 workspace root 内"),
            WorkspaceError::PathNotFound => ("path_denied", "cwd 不存在"),
            WorkspaceError::NotDirectory => ("path_denied", "cwd 不是目录"),
            WorkspaceError::NotRegularFile => ("path_denied", "cwd 不是目录"),
            WorkspaceError::EmptyPath => ("invalid_cwd", "cwd 不能为空"),
            _ => ("invalid_cwd", "cwd 无效"),
        };
        failure(tool_call_id, kind, message, &self.limits, None)
    }
}

fn combine_output(stdout: String, stderr: String) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout,
        (true, false) => format!("[stderr]\n{stderr}"),
        (false, false) => format!("{stdout}\n[stderr]\n{stderr}"),
    }
}

fn failure(
    tool_call_id: ToolCallId,
    kind: &str,
    message: &str,
    limits: &TerminalLimits,
    exit_code: Option<i32>,
) -> ToolResult {
    ToolResult::failure(
        tool_call_id,
        TOOL_NAME,
        kind,
        message,
        limits.max_output_limit.max(1),
        exit_code,
    )
}

fn failure_with_output(
    tool_call_id: ToolCallId,
    kind: &str,
    message: &str,
    stdout: crate::process::BoundedOutput,
    stderr: crate::process::BoundedOutput,
    request: &TerminalRequest,
) -> ToolResult {
    let output = combine_output(stdout.content, stderr.content);
    let mut result = ToolResult::failure(
        tool_call_id,
        TOOL_NAME,
        kind,
        if output.is_empty() { message } else { &output },
        request.output_limit,
        None,
    );
    result.truncated |= stdout.truncated || stderr.truncated;
    result
}
