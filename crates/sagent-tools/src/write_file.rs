//! Workspace 内受审批的原子文本写入工具。

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use sagent_types::ToolCallId;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::{ToolResult, WorkspaceError, WorkspaceRoot};

const TOOL_NAME: &str = "write_file";
const DEFAULT_MAX_CONTENT_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_OUTPUT_CHARS: usize = 4096;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// `write_file` 的模型输入；覆盖已有文件必须显式选择。
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteFileRequest {
    /// 相对 workspace root 的目标文件路径。
    pub path: String,
    /// 将作为 UTF-8 文本写入的完整内容。
    pub content: String,
    /// 是否允许替换一个已存在的普通文件；省略时拒绝覆盖。
    #[serde(default)]
    pub overwrite: bool,
}

impl WriteFileRequest {
    /// 构造默认不覆盖的写入请求，调用方必须显式调用 `with_overwrite` 才能替换文件。
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
            overwrite: false,
        }
    }

    /// 显式记录覆盖意图；实际执行前仍会经过审批与 workspace 校验。
    pub fn with_overwrite(mut self) -> Self {
        self.overwrite = true;
        self
    }
}

/// `write_file` 的输入与模型可见输出上限。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WriteFileLimits {
    /// 单次允许写入的 UTF-8 字节数。
    pub max_content_bytes: usize,
    /// 成功或失败摘要的最大字符数。
    pub max_output_chars: usize,
}

impl Default for WriteFileLimits {
    fn default() -> Self {
        Self {
            max_content_bytes: DEFAULT_MAX_CONTENT_BYTES,
            max_output_chars: DEFAULT_MAX_OUTPUT_CHARS,
        }
    }
}

/// 绑定一个 workspace root 的原子写入服务。
#[derive(Debug, Clone)]
pub struct WriteFileService {
    workspace: WorkspaceRoot,
    limits: WriteFileLimits,
}

impl WriteFileService {
    /// 将 workspace 和写入限制绑定为无状态工具服务。
    pub fn new(workspace: WorkspaceRoot, limits: WriteFileLimits) -> Self {
        Self { workspace, limits }
    }

    /// 在同一目录的临时 sibling 文件中写入、sync 后提交。
    ///
    /// 目标路径、临时文件名和内容都不会回显给模型。取消在 commit 前必定删除临时文件；
    /// commit 后取消不再谎称“未写入”，因为 rename 已经使新内容对工作区可见。
    pub async fn write(
        &self,
        tool_call_id: ToolCallId,
        request: WriteFileRequest,
        cancellation: CancellationToken,
    ) -> ToolResult {
        if cancellation.is_cancelled() {
            return failure(tool_call_id, "cancelled", "文件写入已取消", &self.limits);
        }
        if request.content.len() > self.limits.max_content_bytes {
            return failure(
                tool_call_id,
                "content_too_large",
                "写入内容超过允许的大小限制",
                &self.limits,
            );
        }
        let target = match self.workspace.resolve_write_target(&request.path) {
            Ok(target) => target,
            Err(error) => return self.workspace_error(tool_call_id, error),
        };
        let exists = match tokio::fs::try_exists(&target).await {
            Ok(exists) => exists,
            Err(error) => return io_failure(tool_call_id, error, &self.limits),
        };
        if exists && !request.overwrite {
            return failure(
                tool_call_id,
                "already_exists",
                "目标文件已存在；请显式设置 overwrite",
                &self.limits,
            );
        }

        let temporary = temporary_sibling(&target);
        let write_result = self
            .write_and_commit(
                &temporary,
                &target,
                request.content.as_bytes(),
                request.overwrite,
                &cancellation,
            )
            .await;
        // 无论失败路径来自写入、取消还是 rename，都要尝试删除私有临时文件。成功 commit
        // 后它已被 rename，此操作自然是 no-op；绝不把临时路径写入 ToolResult。
        let _ = tokio::fs::remove_file(&temporary).await;
        match write_result {
            Ok(()) => ToolResult::success(
                tool_call_id,
                TOOL_NAME,
                format!("已写入 {}（{} bytes）", request.path, request.content.len()),
                self.limits.max_output_chars.max(1),
                None,
            ),
            Err(WriteError::Cancelled) => {
                failure(tool_call_id, "cancelled", "文件写入已取消", &self.limits)
            }
            Err(WriteError::AlreadyExists) => failure(
                tool_call_id,
                "already_exists",
                "目标文件已存在；请显式设置 overwrite",
                &self.limits,
            ),
            Err(WriteError::Io(error)) => io_failure(tool_call_id, error, &self.limits),
        }
    }

    async fn write_and_commit(
        &self,
        temporary: &Path,
        target: &Path,
        content: &[u8],
        overwrite: bool,
        cancellation: &CancellationToken,
    ) -> Result<(), WriteError> {
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temporary)
            .await
            .map_err(WriteError::Io)?;
        let mut file = file;
        tokio::select! {
            _ = cancellation.cancelled() => return Err(WriteError::Cancelled),
            result = file.write_all(content) => result.map_err(WriteError::Io)?,
        }
        tokio::select! {
            _ = cancellation.cancelled() => return Err(WriteError::Cancelled),
            result = file.sync_all() => result.map_err(WriteError::Io)?,
        }
        drop(file);
        if cancellation.is_cancelled() {
            return Err(WriteError::Cancelled);
        }
        if overwrite {
            // rename 是同目录原子提交边界；临时文件与目标在同一卷上，避免 copy/delete
            // 暴露半写内容。平台无法替换时返回受控 write_failed，而不先删除旧文件。
            tokio::fs::rename(temporary, target)
                .await
                .map_err(WriteError::Io)
        } else {
            // hard link 的“目标必须不存在”由文件系统原子保证，避免两个 no-overwrite
            // 调用都在检查后抢占同一路径。随后删除临时名，内容仍由新链接保留。
            tokio::fs::hard_link(temporary, target)
                .await
                .map_err(|error| match error.kind() {
                    ErrorKind::AlreadyExists => WriteError::AlreadyExists,
                    _ => WriteError::Io(error),
                })?;
            Ok(())
        }
    }

    fn workspace_error(&self, tool_call_id: ToolCallId, error: WorkspaceError) -> ToolResult {
        let (kind, message) = match error {
            WorkspaceError::EmptyPath => ("invalid_path", "文件路径不能为空"),
            WorkspaceError::PathDenied => ("path_denied", "文件路径不在 workspace root 内"),
            WorkspaceError::ParentNotFound => ("parent_not_found", "目标父目录不存在"),
            WorkspaceError::NotRegularFile => ("not_file", "目标不是普通文件"),
            WorkspaceError::NotDirectory => ("not_directory", "目标父目录不是目录"),
            WorkspaceError::PathNotFound => ("not_found", "目标文件不存在"),
            WorkspaceError::RootNotFound | WorkspaceError::RootNotDirectory => {
                ("workspace_invalid", "workspace root 无效")
            }
            WorkspaceError::Io(_) => ("write_failed", "无法访问文件路径"),
        };
        failure(tool_call_id, kind, message, &self.limits)
    }
}

enum WriteError {
    Cancelled,
    AlreadyExists,
    Io(std::io::Error),
}

fn temporary_sibling(target: &Path) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    target.with_file_name(format!(
        ".{name}.sagent-write-{}-{sequence}.tmp",
        std::process::id()
    ))
}

fn failure(
    tool_call_id: ToolCallId,
    kind: &str,
    message: &str,
    limits: &WriteFileLimits,
) -> ToolResult {
    ToolResult::failure(
        tool_call_id,
        TOOL_NAME,
        kind,
        message,
        limits.max_output_chars.max(1),
        None,
    )
}

fn io_failure(
    tool_call_id: ToolCallId,
    error: std::io::Error,
    limits: &WriteFileLimits,
) -> ToolResult {
    let kind = match error.kind() {
        ErrorKind::PermissionDenied => "permission_denied",
        _ => "write_failed",
    };
    failure(tool_call_id, kind, "写入文件失败", limits)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sagent_types::ToolCallId;
    use tokio_util::sync::CancellationToken;

    use super::{WriteFileLimits, WriteFileRequest, WriteFileService};
    use crate::WorkspaceRoot;

    #[tokio::test]
    async fn creates_and_only_explicitly_overwrites_regular_files() {
        let root = temp_dir();
        let service = WriteFileService::new(WorkspaceRoot::new(&root).unwrap(), Default::default());
        let created = service
            .write(
                ToolCallId::new(),
                WriteFileRequest::new("note.txt", "第一版"),
                CancellationToken::new(),
            )
            .await;
        assert!(created.ok);
        assert_eq!(fs::read_to_string(root.join("note.txt")).unwrap(), "第一版");

        let denied = service
            .write(
                ToolCallId::new(),
                WriteFileRequest::new("note.txt", "第二版"),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(denied.error_kind.as_deref(), Some("already_exists"));
        let replaced = service
            .write(
                ToolCallId::new(),
                WriteFileRequest::new("note.txt", "第二版").with_overwrite(),
                CancellationToken::new(),
            )
            .await;
        assert!(replaced.ok);
        assert_eq!(fs::read_to_string(root.join("note.txt")).unwrap(), "第二版");
        cleanup(root);
    }

    #[tokio::test]
    async fn rejects_missing_parent_escape_and_pre_cancelled_write() {
        let root = temp_dir();
        let service = WriteFileService::new(WorkspaceRoot::new(&root).unwrap(), Default::default());
        let parent = service
            .write(
                ToolCallId::new(),
                WriteFileRequest::new("missing/note.txt", "x"),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(parent.error_kind.as_deref(), Some("parent_not_found"));
        let escaped = service
            .write(
                ToolCallId::new(),
                WriteFileRequest::new("../outside.txt", "x"),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(escaped.error_kind.as_deref(), Some("path_denied"));
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = service
            .write(
                ToolCallId::new(),
                WriteFileRequest::new("cancelled.txt", "x"),
                cancellation,
            )
            .await;
        assert_eq!(cancelled.error_kind.as_deref(), Some("cancelled"));
        assert!(!root.join("cancelled.txt").exists());
        cleanup(root);
    }

    #[tokio::test]
    async fn concurrent_no_overwrite_calls_leave_one_complete_file_and_no_temp_files() {
        let root = temp_dir();
        let service = WriteFileService::new(WorkspaceRoot::new(&root).unwrap(), Default::default());
        let first = service.write(
            ToolCallId::new(),
            WriteFileRequest::new("race.txt", "first"),
            CancellationToken::new(),
        );
        let second = service.write(
            ToolCallId::new(),
            WriteFileRequest::new("race.txt", "second"),
            CancellationToken::new(),
        );
        let (first, second) = tokio::join!(first, second);
        assert_eq!(usize::from(first.ok) + usize::from(second.ok), 1);
        assert!(matches!(
            fs::read_to_string(root.join("race.txt")).unwrap().as_str(),
            "first" | "second"
        ));
        assert!(fs::read_dir(&root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("sagent-write")
        }));
        cleanup(root);
    }

    #[tokio::test]
    async fn applies_content_limit_before_creating_a_file() {
        let root = temp_dir();
        let service = WriteFileService::new(
            WorkspaceRoot::new(&root).unwrap(),
            WriteFileLimits {
                max_content_bytes: 2,
                ..Default::default()
            },
        );
        let result = service
            .write(
                ToolCallId::new(),
                WriteFileRequest::new("large.txt", "三个"),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(result.error_kind.as_deref(), Some("content_too_large"));
        assert!(!root.join("large.txt").exists());
        cleanup(root);
    }

    fn temp_dir() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "sagent-tools-write-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn cleanup(path: std::path::PathBuf) {
        let _ = fs::remove_dir_all(path);
    }
}
