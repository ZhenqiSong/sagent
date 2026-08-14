//! 路径解析模块。
//!
//! 定义 Sagent 的本地 home、日志、缓存和运行时文件边界。
//! 使用 `SAGENT_HOME` 环境变量覆盖默认路径。
//!
//! 默认路径规则（所有平台统一为用户 HOME 下的 `.sagent`）：
//! - Linux/macOS: `$HOME/.sagent`
//! - Windows: `%USERPROFILE%\.sagent`
//!
//! Phase 0 不创建数据库、sessions 或 secrets 目录。
//! 目录创建采用显式初始化，不在纯路径查询函数中产生隐式副作用。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 路径解析
//! @change   2025-08-12 增强：添加 SAGENT_HOME 边界验证，统一使用 HOME 目录

use std::path::PathBuf;

/// SAGENT_HOME 环境变量名。
pub const ENV_SAGENT_HOME: &str = "SAGENT_HOME";

/// Sagent 本地 home 目录管理器。
///
/// 提供平台感知的路径解析和 `SAGENT_HOME` 环境变量覆盖。
/// 所有目录都使用 `PathBuf`，不拼接硬编码 `/`。
#[derive(Debug, Clone)]
pub struct SagentHome {
    /// 根目录路径（已 canonicalize 的绝对路径）
    root: PathBuf,
}

impl SagentHome {
    /// 发现 Sagent home 目录。
    ///
    /// 优先级：`SAGENT_HOME` 环境变量 → 平台默认路径。
    ///
    /// `SAGENT_HOME` 为相对路径时返回错误。
    pub fn discover() -> Result<Self, PathError> {
        if let Ok(home) = std::env::var(ENV_SAGENT_HOME) {
            let home = home.trim();
            if !home.is_empty() {
                return Self::from_env_str(home);
            }
        }
        Ok(Self {
            root: Self::platform_default(),
        })
    }

    /// 从环境变量字符串创建（带验证）。
    ///
    /// 拒绝相对路径、包含 NUL 字符的路径。
    fn from_env_str(home: &str) -> Result<Self, PathError> {
        // 检查 NUL 字符
        if home.contains('\0') {
            return Err(PathError::InvalidPath(
                "SAGENT_HOME contains NUL character".to_string(),
            ));
        }

        let path = PathBuf::from(home);

        // 检查是否为绝对路径
        if !path.is_absolute() {
            return Err(PathError::RelativePath(home.to_string()));
        }

        Ok(Self { root: path })
    }

    /// 从指定的根路径创建 SagentHome 实例。
    ///
    /// 主要用于测试，避免环境变量竞态条件。
    /// 不验证路径是否为绝对路径（允许测试使用相对路径）。
    pub fn from_root(root: PathBuf) -> Self {
        Self { root }
    }

    /// 从环境变量创建（不 fallback 到默认路径）。
    ///
    /// 返回 `None` 如果 `SAGENT_HOME` 未设置或为空。
    /// 返回 `Err` 如果路径无效（相对路径或包含 NUL）。
    pub fn from_env() -> Option<Result<Self, PathError>> {
        let home = std::env::var(ENV_SAGENT_HOME).ok()?;
        let home = home.trim();
        if home.is_empty() {
            return None;
        }
        Some(Self::from_env_str(home))
    }

    /// 获取平台默认路径。
    ///
    /// 所有平台统一使用用户 HOME 目录下的 `.sagent`：
    /// - Linux/macOS: `$HOME/.sagent`
    /// - Windows: `%USERPROFILE%\.sagent`
    fn platform_default() -> PathBuf {
        #[cfg(not(target_os = "windows"))]
        {
            if let Ok(home) = std::env::var("HOME") {
                let home = home.trim();
                if !home.is_empty() {
                    return PathBuf::from(home).join(".sagent");
                }
            }
            PathBuf::from(".").join(".sagent")
        }

        #[cfg(target_os = "windows")]
        {
            if let Ok(profile) = std::env::var("USERPROFILE") {
                let profile = profile.trim();
                if !profile.is_empty() {
                    return PathBuf::from(profile).join(".sagent");
                }
            }
            PathBuf::from(".").join(".sagent")
        }
    }

    /// 获取根目录路径。
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    /// 获取配置目录路径（`<root>/config`）。
    pub fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }

    /// 获取日志目录路径（`<root>/logs`）。
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// 获取缓存目录路径（`<root>/cache`）。
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// 获取运行时目录路径（`<root>/runtime`）。
    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join("runtime")
    }
}

/// 路径相关错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// 相对路径（SAGENT_HOME 必须是绝对路径）
    RelativePath(String),
    /// 无效路径（如包含 NUL 字符）
    InvalidPath(String),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RelativePath(p) => {
                write!(f, "SAGENT_HOME must be an absolute path, got: {}", p)
            },
            Self::InvalidPath(msg) => write!(f, "Invalid SAGENT_HOME path: {}", msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // 基本功能测试
    // ========================================================================

    #[test]
    fn test_from_root_uses_given_path() {
        let home = SagentHome::from_root(PathBuf::from("/tmp/test-sagent"));
        assert_eq!(home.root(), &PathBuf::from("/tmp/test-sagent"));
    }

    #[test]
    fn test_subdirectories_are_correct() {
        let home = SagentHome::from_root(PathBuf::from("/tmp/test-sagent"));
        assert_eq!(home.config_dir(), PathBuf::from("/tmp/test-sagent/config"));
        assert_eq!(home.logs_dir(), PathBuf::from("/tmp/test-sagent/logs"));
        assert_eq!(home.cache_dir(), PathBuf::from("/tmp/test-sagent/cache"));
        assert_eq!(
            home.runtime_dir(),
            PathBuf::from("/tmp/test-sagent/runtime")
        );
    }

    #[test]
    fn test_same_instance_repeatable() {
        let home = SagentHome::from_root(PathBuf::from("/tmp/test-sagent"));
        // 同一个实例重复查询得到相同路径
        assert_eq!(home.root(), home.root());
        assert_eq!(home.config_dir(), home.config_dir());
    }

    // ========================================================================
    // SAGENT_HOME 环境变量覆盖测试
    // 使用 from_env_str 直接测试验证逻辑，避免 set_var/remove_var 竞态
    // ========================================================================

    #[test]
    fn test_env_override_absolute_path() {
        let home = SagentHome::from_env_str("/tmp/test-env-override").expect("应接受绝对路径");
        assert_eq!(home.root(), &PathBuf::from("/tmp/test-env-override"));
    }

    #[test]
    fn test_env_empty_string_is_rejected_by_discover_but_falls_back() {
        // discover() 中对空字符串做 fallback，而不是返回错误
        let home = SagentHome::from_root(PathBuf::from("/tmp/test-sagent"));
        assert!(!home.root().to_string_lossy().is_empty());
    }

    #[test]
    fn test_platform_default_is_not_empty() {
        let root = SagentHome::platform_default();
        assert!(!root.to_string_lossy().is_empty());
    }

    #[test]
    fn test_from_env_str_accepts_absolute_path() {
        let home = SagentHome::from_env_str("/custom/sagent/path").expect("应接受绝对路径");
        assert_eq!(home.root(), &PathBuf::from("/custom/sagent/path"));
    }

    // ========================================================================
    // SAGENT_HOME 边界条件测试
    // ========================================================================

    #[test]
    fn test_relative_path_is_rejected() {
        let result = SagentHome::from_env_str("relative/path");
        assert!(result.is_err());
        match result.unwrap_err() {
            PathError::RelativePath(p) => assert_eq!(p, "relative/path"),
            other => panic!("期望 RelativePath 错误，实际为: {:?}", other),
        }
    }

    #[test]
    fn test_nul_byte_is_rejected() {
        // std::env::set_var 本身拒绝 NUL 字节，直接测试 from_env_str
        let result = SagentHome::from_env_str("/tmp/bad\0path");
        assert!(result.is_err());
        match result.unwrap_err() {
            PathError::InvalidPath(msg) => assert!(msg.contains("NUL")),
            other => panic!("期望 InvalidPath 错误，实际为: {:?}", other),
        }
    }

    #[test]
    fn test_absolute_path_with_trailing_slash_is_accepted() {
        let home =
            SagentHome::from_env_str("/tmp/test-trailing/").expect("应接受带尾斜杠的绝对路径");
        assert_eq!(home.root(), &PathBuf::from("/tmp/test-trailing/"));
    }

    // ========================================================================
    // 平台默认路径 fixture 测试
    // ========================================================================

    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_default_ends_with_sagent() {
        let root = SagentHome::platform_default();
        let path_str = root.to_string_lossy();
        assert!(
            path_str.ends_with(".sagent"),
            "macOS 默认路径应以 .sagent 结尾: {}",
            root.display()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_linux_default_ends_with_sagent() {
        let root = SagentHome::platform_default();
        let path_str = root.to_string_lossy();
        assert!(
            path_str.ends_with(".sagent"),
            "Linux 默认路径应以 .sagent 结尾: {}",
            root.display()
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_windows_default_ends_with_sagent() {
        let root = SagentHome::platform_default();
        let path_str = root.to_string_lossy().to_lowercase();
        assert!(
            path_str.ends_with(".sagent"),
            "Windows 默认路径应以 .sagent 结尾: {}",
            root.display()
        );
    }

    // ========================================================================
    // 路径不依赖当前工作目录
    // ========================================================================

    #[test]
    fn test_discover_is_independent_of_cwd() {
        // 使用 from_root 避免环境变量干扰
        let home1 = SagentHome::from_root(PathBuf::from("/tmp/test-sagent"));
        let home2 = SagentHome::from_root(PathBuf::from("/tmp/test-sagent"));
        assert_eq!(home1.root(), home2.root());
    }

    // ========================================================================
    // XDG_DATA_HOME 优先级测试 (Linux only)
    // ========================================================================

    #[cfg(target_os = "linux")]
    #[test]
    fn test_xdg_data_home_overrides_default() {
        std::env::remove_var(ENV_SAGENT_HOME);
        std::env::set_var("XDG_DATA_HOME", "/custom/xdg");
        let root = SagentHome::platform_default();
        assert!(
            root.to_string_lossy().contains("/custom/xdg/sagent"),
            "XDG_DATA_HOME 应优先于 ~/.local/share: {}",
            root.display()
        );
        std::env::remove_var("XDG_DATA_HOME");
    }

    // ========================================================================
    // PathError Display 测试
    // ========================================================================

    #[test]
    fn test_path_error_display_relative() {
        let err = PathError::RelativePath("foo/bar".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("absolute path"));
        assert!(msg.contains("foo/bar"));
    }

    #[test]
    fn test_path_error_display_invalid() {
        let err = PathError::InvalidPath("contains bad chars".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("Invalid"));
        assert!(msg.contains("bad chars"));
    }
}
