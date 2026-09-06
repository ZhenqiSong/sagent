//! Workspace root 和路径安全校验。

use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

/// 文件工具允许访问的根目录。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WorkspaceRoot {
    root: PathBuf,
}

/// Workspace 路径解析错误。
#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum WorkspaceError {
    #[error("workspace root 不存在")]
    RootNotFound,
    #[error("workspace root 不是目录")]
    RootNotDirectory,
    #[error("路径不能为空")]
    EmptyPath,
    #[error("路径不在 workspace root 内")]
    PathDenied,
    #[error("目标路径不存在")]
    PathNotFound,
    #[error("目标不是普通文件")]
    NotRegularFile,
    #[error("目标不是目录")]
    NotDirectory,
    #[error("文件系统操作失败：{0:?}")]
    Io(ErrorKind),
}

impl WorkspaceRoot {
    /// 创建 root，并在初始化时固定其 canonical 路径。
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        let root = root.into();
        let metadata = fs::metadata(&root).map_err(|error| match error.kind() {
            ErrorKind::NotFound => WorkspaceError::RootNotFound,
            kind => WorkspaceError::Io(kind),
        })?;
        if !metadata.is_dir() {
            return Err(WorkspaceError::RootNotDirectory);
        }
        let root = fs::canonicalize(root).map_err(|error| WorkspaceError::Io(error.kind()))?;
        Ok(Self { root })
    }

    /// 返回 canonical 化后的安全根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 解析请求路径，并拒绝 root 外路径与逃逸的符号链接。
    pub fn resolve(&self, requested: impl AsRef<Path>) -> Result<PathBuf, WorkspaceError> {
        self.resolve_kind(requested, false)
    }

    /// 解析可作为 terminal `cwd` 的目录。
    pub fn resolve_directory(
        &self,
        requested: impl AsRef<Path>,
    ) -> Result<PathBuf, WorkspaceError> {
        self.resolve_kind(requested, true)
    }

    fn resolve_kind(
        &self,
        requested: impl AsRef<Path>,
        directory: bool,
    ) -> Result<PathBuf, WorkspaceError> {
        let requested = requested.as_ref();
        if requested.as_os_str().is_empty() {
            return Err(WorkspaceError::EmptyPath);
        }

        let joined = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            self.root.join(requested)
        };
        if !lexically_within(&joined, &self.root) {
            return Err(WorkspaceError::PathDenied);
        }

        let canonical = fs::canonicalize(&joined).map_err(|error| match error.kind() {
            ErrorKind::NotFound => WorkspaceError::PathNotFound,
            kind => WorkspaceError::Io(kind),
        })?;
        if !canonical.starts_with(&self.root) {
            return Err(WorkspaceError::PathDenied);
        }

        let metadata = fs::metadata(&canonical).map_err(|error| match error.kind() {
            ErrorKind::NotFound => WorkspaceError::PathNotFound,
            kind => WorkspaceError::Io(kind),
        })?;
        if directory && !metadata.is_dir() {
            return Err(WorkspaceError::NotDirectory);
        }
        if !directory && !metadata.is_file() {
            return Err(WorkspaceError::NotRegularFile);
        }
        Ok(canonical)
    }
}

/// 在不访问文件系统的情况下折叠 `.` 和 `..`，用于先做 root containment 检查。
fn lexically_within(path: &Path, root: &Path) -> bool {
    let normalized = lexical_normalize(path);
    normalized.starts_with(root)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::{WorkspaceError, WorkspaceRoot};
    use std::fs;

    #[test]
    fn resolves_only_regular_files_inside_root() {
        let directory = tempfile_directory();
        fs::write(directory.join("ok.txt"), "ok").unwrap();
        let root = WorkspaceRoot::new(&directory).unwrap();
        assert_eq!(
            root.resolve("ok.txt").unwrap(),
            fs::canonicalize(directory.join("ok.txt")).unwrap()
        );
        assert_eq!(
            root.resolve_directory(".").unwrap(),
            fs::canonicalize(&directory).unwrap()
        );
        assert_eq!(
            root.resolve_directory("ok.txt"),
            Err(WorkspaceError::NotDirectory)
        );
        assert_eq!(
            root.resolve("missing.txt"),
            Err(WorkspaceError::PathNotFound)
        );
        assert_eq!(root.resolve("."), Err(WorkspaceError::NotRegularFile));
        cleanup(directory);
    }

    #[test]
    fn rejects_empty_and_escaping_paths() {
        let directory = tempfile_directory();
        let root = WorkspaceRoot::new(&directory).unwrap();
        assert_eq!(root.resolve(""), Err(WorkspaceError::EmptyPath));
        assert_eq!(
            root.resolve("../outside.txt"),
            Err(WorkspaceError::PathDenied)
        );
        cleanup(directory);
    }

    fn tempfile_directory() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "sagent-tools-workspace-{}-{}",
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
