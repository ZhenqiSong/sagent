//! Workspace 内 UTF-8 文件读取工具。

use std::path::Path;

use sagent_types::ToolCallId;
use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::{TRUNCATION_MARKER, ToolResult, WorkspaceError, WorkspaceRoot};

const TOOL_NAME: &str = "read_file";
const DEFAULT_OFFSET: usize = 1;
const DEFAULT_LIMIT: usize = 2000;
const DEFAULT_MAX_FILE_BYTES: u64 = 1024 * 1024;
const DEFAULT_MAX_OUTPUT_CHARS: usize = 100_000;
const DEFAULT_MAX_LINES: usize = 2000;
const DEFAULT_MAX_LINE_CHARS: usize = 2000;
const READ_CHUNK_SIZE: usize = 8192;

/// `read_file` 的输入参数；offset 与 limit 都是 1-based/正数语义。
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
pub struct ReadFileRequest {
    pub path: String,
    #[serde(default = "default_offset")]
    pub offset: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

impl ReadFileRequest {
    /// 创建规范化的读取请求；零值会被提升为最小有效值。
    pub fn new(path: impl Into<String>, offset: usize, limit: usize) -> Self {
        Self {
            path: path.into(),
            offset: offset.max(1),
            limit: limit.max(1),
        }
    }

    fn normalized(self, max_lines: usize) -> Self {
        Self {
            path: self.path,
            offset: self.offset.max(1),
            limit: self.limit.max(1).min(max_lines.max(1)),
        }
    }
}

fn default_offset() -> usize {
    DEFAULT_OFFSET
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

/// read_file 的文件和输出限制。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReadFileLimits {
    /// 单文件最多读取的字节数。
    pub max_file_bytes: u64,
    /// 返回内容最多保留的字符数。
    pub max_output_chars: usize,
    /// 单次最多返回的行数。
    pub max_lines: usize,
    /// 单行最多保留的字符数。
    pub max_line_chars: usize,
}

impl Default for ReadFileLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_output_chars: DEFAULT_MAX_OUTPUT_CHARS,
            max_lines: DEFAULT_MAX_LINES,
            max_line_chars: DEFAULT_MAX_LINE_CHARS,
        }
    }
}

/// 绑定一个 workspace root 的文件读取服务。
#[derive(Debug, Clone)]
pub struct ReadFileService {
    workspace: WorkspaceRoot,
    limits: ReadFileLimits,
}

impl ReadFileService {
    /// 将 workspace 与读取限制绑定为一个无状态服务。
    pub fn new(workspace: WorkspaceRoot, limits: ReadFileLimits) -> Self {
        Self { workspace, limits }
    }

    /// 返回该读取服务绑定的 workspace。
    pub fn workspace(&self) -> &WorkspaceRoot {
        &self.workspace
    }

    /// 返回当前读取限制。
    pub fn limits(&self) -> &ReadFileLimits {
        &self.limits
    }

    /// 读取文件；该函数只访问 workspace，不执行 shell、不写 Store。
    pub async fn read(
        &self,
        tool_call_id: ToolCallId,
        request: ReadFileRequest,
        cancellation: CancellationToken,
    ) -> ToolResult {
        let request = request.normalized(self.limits.max_lines);
        if cancellation.is_cancelled() {
            return failure(tool_call_id, "cancelled", "文件读取已取消", &self.limits);
        }

        let path = match self.workspace.resolve(&request.path) {
            Ok(path) => path,
            Err(error) => return self.workspace_error(tool_call_id, error),
        };

        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) => return io_failure(tool_call_id, error, &self.limits),
        };
        if metadata.len() > self.limits.max_file_bytes {
            return failure(
                tool_call_id,
                "file_too_large",
                "文件超过允许的大小限制",
                &self.limits,
            );
        }
        if has_binary_extension(&path) {
            return failure(
                tool_call_id,
                "binary_file",
                "不能将二进制文件作为文本读取",
                &self.limits,
            );
        }

        let bytes = match read_bytes(&path, &cancellation, self.limits.max_file_bytes).await {
            Ok(bytes) => bytes,
            Err(ReadBytesError::Cancelled) => {
                return failure(tool_call_id, "cancelled", "文件读取已取消", &self.limits);
            }
            Err(ReadBytesError::TooLarge) => {
                return failure(
                    tool_call_id,
                    "file_too_large",
                    "文件超过允许的大小限制",
                    &self.limits,
                );
            }
            Err(ReadBytesError::Io(error)) => return io_failure(tool_call_id, error, &self.limits),
        };

        if looks_binary(&bytes) {
            return failure(
                tool_call_id,
                "binary_file",
                "不能将二进制文件作为文本读取",
                &self.limits,
            );
        }
        let text = match String::from_utf8(bytes) {
            Ok(text) => text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned(),
            Err(_) => {
                return failure(
                    tool_call_id,
                    "binary_file",
                    "文件不是有效的 UTF-8 文本",
                    &self.limits,
                );
            }
        };

        let content = format_page(&text, &request, &self.limits);
        let (content, truncated) = truncate_at_line_boundary(content, self.limits.max_output_chars);
        let mut result = ToolResult::success(
            tool_call_id,
            TOOL_NAME,
            content,
            self.limits.max_output_chars,
            None,
        );
        result.truncated |= truncated;
        result
    }

    fn workspace_error(&self, tool_call_id: ToolCallId, error: WorkspaceError) -> ToolResult {
        let (kind, message) = match error {
            WorkspaceError::EmptyPath => ("invalid_path", "文件路径不能为空"),
            WorkspaceError::PathDenied => ("path_denied", "文件路径不在 workspace root 内"),
            WorkspaceError::PathNotFound => ("not_found", "文件不存在"),
            WorkspaceError::NotRegularFile | WorkspaceError::NotDirectory => {
                ("not_file", "目标不是普通文件")
            }
            WorkspaceError::RootNotFound | WorkspaceError::RootNotDirectory => {
                ("workspace_invalid", "workspace root 无效")
            }
            WorkspaceError::Io(_) => ("read_failed", "无法访问文件路径"),
        };
        failure(tool_call_id, kind, message, &self.limits)
    }
}

enum ReadBytesError {
    Cancelled,
    TooLarge,
    Io(std::io::Error),
}

async fn read_bytes(
    path: &Path,
    cancellation: &CancellationToken,
    max_file_bytes: u64,
) -> Result<Vec<u8>, ReadBytesError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(ReadBytesError::Io)?;
    let mut bytes = Vec::new();
    let mut buffer = vec![0_u8; READ_CHUNK_SIZE];
    loop {
        if cancellation.is_cancelled() {
            return Err(ReadBytesError::Cancelled);
        }
        let count = tokio::select! {
            _ = cancellation.cancelled() => return Err(ReadBytesError::Cancelled),
            result = file.read(&mut buffer) => result.map_err(ReadBytesError::Io)?,
        };
        if count == 0 {
            return Ok(bytes);
        }
        if bytes.len() as u64 + count as u64 > max_file_bytes {
            return Err(ReadBytesError::TooLarge);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

fn format_page(text: &str, request: &ReadFileRequest, limits: &ReadFileLimits) -> String {
    text.lines()
        .enumerate()
        .skip(request.offset.saturating_sub(1))
        .take(request.limit)
        .map(|(index, line)| {
            let line = truncate_line(line, limits.max_line_chars);
            format!("{}|{line}", index + 1)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_owned();
    }
    line.chars().take(max_chars.max(1)).collect()
}

fn truncate_at_line_boundary(content: String, max_chars: usize) -> (String, bool) {
    if content.chars().count() <= max_chars {
        return (content, false);
    }
    let marker_len = TRUNCATION_MARKER.chars().count();
    if max_chars <= marker_len {
        return (TRUNCATION_MARKER.chars().take(max_chars).collect(), true);
    }
    let budget = max_chars - marker_len;
    let mut kept = String::new();
    for line in content.lines() {
        let addition = line.chars().count() + usize::from(!kept.is_empty());
        if kept.chars().count() + addition > budget {
            break;
        }
        if !kept.is_empty() {
            kept.push('\n');
        }
        kept.push_str(line);
    }
    if kept.is_empty() {
        kept = content.chars().take(budget).collect();
    }
    (format!("{kept}{TRUNCATION_MARKER}"), true)
}

fn has_binary_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "bmp"
                    | "ico"
                    | "webp"
                    | "tiff"
                    | "tif"
                    | "mp4"
                    | "mov"
                    | "avi"
                    | "mkv"
                    | "webm"
                    | "mp3"
                    | "wav"
                    | "ogg"
                    | "flac"
                    | "zip"
                    | "tar"
                    | "gz"
                    | "bz2"
                    | "7z"
                    | "rar"
                    | "xz"
                    | "exe"
                    | "dll"
                    | "so"
                    | "dylib"
                    | "bin"
                    | "o"
                    | "a"
                    | "obj"
                    | "lib"
                    | "msi"
                    | "doc"
                    | "docx"
                    | "xls"
                    | "xlsx"
                    | "ppt"
                    | "pptx"
                    | "ttf"
                    | "otf"
                    | "woff"
                    | "woff2"
                    | "pyc"
                    | "class"
                    | "jar"
                    | "wasm"
                    | "rlib"
                    | "sqlite"
                    | "sqlite3"
                    | "db"
                    | "mdb"
                    | "idx"
                    | "psd"
                    | "ai"
                    | "sketch"
                    | "blend"
                    | "3ds"
                    | "max"
                    | "swf"
                    | "fla"
                    | "lockb"
                    | "dat"
                    | "data"
            )
        })
        .unwrap_or(false)
}

fn looks_binary(bytes: &[u8]) -> bool {
    const MAGIC: &[&[u8]] = &[
        b"\x89PNG\r\n\x1a\n",
        b"\xff\xd8\xff",
        b"GIF87a",
        b"GIF89a",
        b"PK\x03\x04",
        b"\x1f\x8b",
        b"\x7fELF",
        b"MZ",
        b"SQLite format 3\0",
        b"%PDF-",
    ];
    MAGIC.iter().any(|signature| bytes.starts_with(signature)) || bytes.contains(&0)
}

fn failure(
    tool_call_id: ToolCallId,
    error_kind: &str,
    content: &str,
    limits: &ReadFileLimits,
) -> ToolResult {
    ToolResult::failure(
        tool_call_id,
        TOOL_NAME,
        error_kind,
        content,
        limits.max_output_chars.max(1),
        None,
    )
}

fn io_failure(
    tool_call_id: ToolCallId,
    error: std::io::Error,
    limits: &ReadFileLimits,
) -> ToolResult {
    let kind = match error.kind() {
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        _ => "read_failed",
    };
    failure(tool_call_id, kind, "读取文件失败", limits)
}

#[cfg(test)]
mod tests {
    use super::{ReadFileLimits, ReadFileRequest, ReadFileService};
    use crate::WorkspaceRoot;
    use sagent_types::ToolCallId;
    use std::fs;
    use tokio_util::sync::CancellationToken;

    #[tokio::test(flavor = "current_thread")]
    async fn reads_utf8_unicode_with_one_based_pagination() {
        let directory = temp_dir();
        fs::write(
            directory.join("notes.txt"),
            "第一行\n你好，Sagent 🦀\n第三行",
        )
        .unwrap();
        let service =
            ReadFileService::new(WorkspaceRoot::new(&directory).unwrap(), Default::default());
        let result = service
            .read(
                ToolCallId::new(),
                ReadFileRequest::new("notes.txt", 2, 1),
                CancellationToken::new(),
            )
            .await;
        assert!(result.ok);
        assert_eq!(result.content, "2|你好，Sagent 🦀");
        cleanup(directory);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rejects_binary_large_and_outside_files() {
        let directory = temp_dir();
        fs::write(directory.join("bytes.bin"), [0_u8, 159, 146, 150]).unwrap();
        fs::write(directory.join("large.txt"), [b'x'; 32]).unwrap();
        let service = ReadFileService::new(
            WorkspaceRoot::new(&directory).unwrap(),
            ReadFileLimits {
                max_file_bytes: 16,
                ..Default::default()
            },
        );
        let binary = service
            .read(
                ToolCallId::new(),
                ReadFileRequest::new("bytes.bin", 1, 10),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(binary.error_kind.as_deref(), Some("binary_file"));
        let large = service
            .read(
                ToolCallId::new(),
                ReadFileRequest::new("large.txt", 1, 10),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(large.error_kind.as_deref(), Some("file_too_large"));
        let outside = service
            .read(
                ToolCallId::new(),
                ReadFileRequest::new("../outside.txt", 1, 10),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(outside.error_kind.as_deref(), Some("path_denied"));
        cleanup(directory);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn truncates_output_and_honors_pre_cancelled_token() {
        let directory = temp_dir();
        fs::write(directory.join("long.txt"), "11111\n22222\n33333").unwrap();
        let service = ReadFileService::new(
            WorkspaceRoot::new(&directory).unwrap(),
            ReadFileLimits {
                max_output_chars: 12,
                ..Default::default()
            },
        );
        let result = service
            .read(
                ToolCallId::new(),
                ReadFileRequest::new("long.txt", 1, 10),
                CancellationToken::new(),
            )
            .await;
        assert!(result.ok);
        assert!(result.truncated);
        assert!(result.content.chars().count() <= 12);

        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let result = service
            .read(
                ToolCallId::new(),
                ReadFileRequest::new("long.txt", 1, 10),
                cancelled,
            )
            .await;
        assert_eq!(result.error_kind.as_deref(), Some("cancelled"));
        cleanup(directory);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn rejects_symlink_that_escapes_root() {
        use std::os::unix::fs::symlink;
        let directory = temp_dir();
        let outside = temp_dir();
        fs::write(outside.join("secret.txt"), "secret").unwrap();
        symlink(outside.join("secret.txt"), directory.join("link.txt")).unwrap();
        let service =
            ReadFileService::new(WorkspaceRoot::new(&directory).unwrap(), Default::default());
        let result = service
            .read(
                ToolCallId::new(),
                ReadFileRequest::new("link.txt", 1, 10),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(result.error_kind.as_deref(), Some("path_denied"));
        cleanup(directory);
        cleanup(outside);
    }

    fn temp_dir() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "sagent-tools-read-file-{}-{}",
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
