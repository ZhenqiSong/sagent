//! 路径解析模块。
//!
//! 定义 Sagent 的本地 home、日志、缓存和运行时文件边界。
//! 使用 `SAGENT_HOME` 环境变量覆盖默认路径。
//!
//! 默认路径规则（所有平台统一为用户 home 下的 `.sagent`）：
//! - Linux:   `~/.sagent`
//! - macOS:   `~/.sagent`
//! - Windows: `%USERPROFILE%\.sagent`
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 路径解析

use std::path::PathBuf;

/// Sagent 本地 home 目录管理器。
///
/// 提供平台感知的路径解析和 `SAGENT_HOME` 环境变量覆盖。
/// 所有平台默认使用用户 home 下的 `.sagent` 目录。
#[derive(Debug, Clone)]
pub struct SagentHome {
    /// 根目录路径
    root: PathBuf,
}

impl SagentHome {
    /// 发现 Sagent home 目录。
    ///
    /// 优先级：`SAGENT_HOME` 环境变量 → 平台默认路径。
    pub fn discover() -> Self {
        if let Ok(home) = std::env::var("SAGENT_HOME") {
            let home = home.trim();
            if !home.is_empty() {
                return Self {
                    root: PathBuf::from(home),
                };
            }
        }
        Self::default_platform()
    }

    /// 从指定的根路径创建 SagentHome 实例。
    ///
    /// 主要用于测试，避免环境变量竞态条件。
    pub fn from_root(root: PathBuf) -> Self {
        Self { root }
    }

    /// 从环境变量创建（不 fallback 到默认路径）。
    ///
    /// 返回 None 如果 `SAGENT_HOME` 未设置或为空。
    pub fn from_env() -> Option<Self> {
        let home = std::env::var("SAGENT_HOME").ok()?;
        let home = home.trim();
        if home.is_empty() {
            return None;
        }
        Some(Self {
            root: PathBuf::from(home),
        })
    }

    /// 获取平台默认路径。
    ///
    /// 所有平台统一使用用户 home 目录下的 `.sagent`。
    fn default_platform() -> Self {
        let root = if cfg!(target_os = "windows") {
            // Windows: %USERPROFILE%\.sagent
            dirs_fallback("USERPROFILE", ".sagent")
        } else {
            // Linux / macOS: ~/.sagent
            dirs_fallback("HOME", ".sagent")
        };
        Self { root }
    }

    /// 获取根目录路径。
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    /// 获取配置目录路径。
    pub fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }

    /// 获取日志目录路径。
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// 获取缓存目录路径。
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// 获取运行时目录路径。
    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join("runtime")
    }
}

/// 使用环境变量 fallback 构建 home 目录。
///
/// # 参数
///
/// * `env_var` - 环境变量名（如 "HOME"）
/// * `suffix` - 相对于 home 的路径后缀
fn dirs_fallback(env_var: &str, suffix: &str) -> PathBuf {
    if let Ok(home) = std::env::var(env_var) {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home).join(suffix);
        }
    }
    // 最终 fallback
    PathBuf::from(".").join(suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用的 SagentHome，避免环境变量竞态条件。
    fn test_home() -> SagentHome {
        SagentHome::from_root(PathBuf::from("/tmp/test-sagent"))
    }

    #[test]
    fn test_from_root() {
        let home = SagentHome::from_root(PathBuf::from("/tmp/test-sagent"));
        assert_eq!(home.root(), &PathBuf::from("/tmp/test-sagent"));
    }

    #[test]
    fn test_sagent_home_from_env_override() {
        std::env::set_var("SAGENT_HOME", "/tmp/test-env-override");
        let home = SagentHome::discover();
        assert_eq!(home.root(), &PathBuf::from("/tmp/test-env-override"));
        std::env::remove_var("SAGENT_HOME");
    }

    #[test]
    fn test_sagent_home_from_env_empty() {
        std::env::set_var("SAGENT_HOME", "");
        let home = SagentHome::discover();
        // 应该 fallback 到平台默认路径
        assert!(!home.root().to_string_lossy().is_empty());
        std::env::remove_var("SAGENT_HOME");
    }

    #[test]
    fn test_subdirectories() {
        let home = test_home();
        assert_eq!(home.config_dir(), PathBuf::from("/tmp/test-sagent/config"));
        assert_eq!(home.logs_dir(), PathBuf::from("/tmp/test-sagent/logs"));
        assert_eq!(home.cache_dir(), PathBuf::from("/tmp/test-sagent/cache"));
        assert_eq!(
            home.runtime_dir(),
            PathBuf::from("/tmp/test-sagent/runtime")
        );
    }

    #[test]
    fn test_same_process_repeatable() {
        let home = test_home();
        assert_eq!(home.root(), home.root());
    }
}
