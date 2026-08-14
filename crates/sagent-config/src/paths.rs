//! 配置路径解析。
//!
//! 路径规则直接复用 Phase 0 的 `SagentHome`；所有路径均在加载时固定到一个显式的 home 根目录，
//! 避免随当前工作目录变化。
//!
//! @author   songzq
//! @created  2026-08-14
//! @change   2026-08-14 初始版本：Phase 1 Step 1 配置路径

use std::path::{Path, PathBuf};

use sagent_api::paths::{PathError as HomePathError, SagentHome};

/// Sagent home 环境变量名。
pub const ENV_SAGENT_HOME: &str = "SAGENT_HOME";
/// 配置文件名。
pub const CONFIG_FILE_NAME: &str = "config.yaml";

/// 配置文件路径集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    root: PathBuf,
}

impl ConfigPaths {
    /// 从 `SAGENT_HOME` 或平台默认目录发现 home。
    pub fn discover() -> Result<Self, PathError> {
        Ok(Self {
            root: SagentHome::discover()
                .map_err(|error| match error {
                    HomePathError::RelativePath(_) => PathError::RelativePath,
                    HomePathError::InvalidPath(_) => PathError::InvalidPath,
                })?
                .root()
                .clone(),
        })
    }

    /// 从显式根目录创建路径集合，主要用于测试和进程装配。
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 返回 home 根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 返回配置文件路径。
    pub fn config_file(&self) -> PathBuf {
        self.root.join(CONFIG_FILE_NAME)
    }

    /// 将数据库配置路径解析为 home 下的稳定路径。
    pub fn resolve_database_path(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        }
    }
}

/// 配置 home 路径错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// 环境变量中的路径不是绝对路径。
    #[error("SAGENT_HOME 必须是绝对路径")]
    RelativePath,
    /// 路径包含操作系统不允许的 NUL 字符。
    #[error("SAGENT_HOME 包含 NUL 字符")]
    InvalidPath,
}
