
//! sagent 路径管理模块。
//!
//! 负责解析和提供 sagent 运行所需的各类路径，包括：
//! - 用户自定义覆盖路径（通过 `task_local!` 实现协程级隔离）
//! - 平台默认路径（Unix: `~/.sagent`，Windows: `%LOCALAPPDATA%/sagent`）
//! - 多 Profile 回退告警（检测 `activate_profile` 文件并发出警告）

use std::{env, fs, io::{self, Write}, path::PathBuf, sync::atomic::{AtomicBool, Ordering}};
use tokio::task_local;

// 协程级 sagent home 路径覆盖标记。
// 通过 `task_local!` 实现，仅在当前 tokio 任务/协程内有效，
// 子进程启动器应在启动子任务前设置此覆盖。
task_local!{
    static SAGENT_HOME_OVERRIDE: String
}

/// 全局告警标记，确保 `SAGENT_HOME` 回退警告最多输出一次。
static PROFILE_FALLBACK_WARNED: AtomicBool = AtomicBool::new(false);

/// 尝试读取当前协程的 `SAGENT_HOME_OVERRIDE` 值。
///
/// Returns:
/// - `Some(PathBuf)` — 在当前协程内设置了覆盖路径
/// - `None` — 未设置覆盖
fn get_sagent_home_override() -> Option<PathBuf>{
    SAGENT_HOME_OVERRIDE.try_with(|v| {
        v.clone().into()
    }).ok()
}

/// 获取当前用户的家目录路径。
///
/// 使用 `dirs::home_dir()` 获取，失败时回退到 `/home`。
fn get_user_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home"))
}

/// 计算平台默认的 sagent home 路径。
///
/// - **Unix 系**: `~/.sagent`
/// - **Windows**: `%LOCALAPPDATA%/sagent`（失败时回退到 `~/AppData/Local/sagent`）
///
/// # Examples
///
/// ```ignore
/// let path = get_platform_default_sagent_home();
/// assert!(path.is_absolute());
/// ```
fn get_platform_default_sagent_home() -> PathBuf {
    let home = get_user_home();
    
    if cfg!(target_os = "windows") {
        // Windows 习惯用 %APPDATA%
        env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or(home.join("AppData").join("Local"))
            .join("sagent")
    } else {
        home.join(".sagent")
    }
}


/// 获取 sagent 数据根目录路径。
///
/// 路径解析优先级（高 → 低）：
/// 1. `SAGENT_HOME_OVERRIDE`（协程级覆盖，通过 `tokio::task_local` 设置）
/// 2. 平台默认路径（Unix: `~/.sagent`，Windows: `%LOCALAPPDATA%/sagent`）
///
/// # 回退告警
///
/// 当未设置覆盖且 `~/.sagent/activate_profile` 中存在非 default 的 Profile 时，
/// 会在 stderr 输出一次告警（全局仅一次），提醒用户数据可能写入错误位置。
///
/// # Examples
///
/// ```ignore
/// use sagent_common::path::get_sagent_home;
/// let home = get_sagent_home();
/// assert!(home.is_absolute());
/// ```
pub fn get_sagent_home() -> PathBuf {
    if let Some(override_home) = get_sagent_home_override(){
        return override_home;
    }

    if !PROFILE_FALLBACK_WARNED.swap(true, Ordering::Relaxed){
        let fallback_home = get_platform_default_sagent_home();
        let activate_path = fallback_home.join("activate_profile");

        let active = fs::read_to_string(&activate_path).map(|s| s.trim().to_owned()).unwrap_or_default();

        if !active.is_empty() && active != "default"{
            let msg = format!(
                "[SAGENT_HOME 回退] SAGENT_HOME 未设置，但当前激活的配置是 {active:?}。\
                 回退到默认路径 {:?}，这不是你期望的配置 {active:?}。\
                 此进程写入的数据将落在错误的配置下。子进程启动器应显式传入 SAGENT_HOME 环境变量。",
                 fallback_home,
            );
            let _ = writeln!(io::stderr(), "{msg}");
       }
    }
    get_platform_default_sagent_home()
}

#[cfg(test)]
mod tests {
    //! `path` 模块单元测试。
    //!
    //! 覆盖路径解析的三种场景：默认路径、协程级覆盖路径、覆盖优先级验证。
    //! 全局共享状态（`PROFILE_FALLBACK_WARNED`）在测试间可能相互影响，
    //! 但测试中不涉及该状态的修改，故可安全并行执行。
    use super::*;

    // ── get_user_home ─────────────────────────────────────────────────

    #[test]
    fn test_get_user_home_returns_existing_path() {
        let home = get_user_home();
        assert!(!home.as_os_str().is_empty(), "home path should not be empty");
        assert!(home.is_absolute(), "home path should be absolute");
    }

    // ── get_platform_default_sagent_home ──────────────────────────────

    #[test]
    fn test_platform_default_sagent_home_ends_with_sagent() {
        let path = get_platform_default_sagent_home();
        #[cfg(not(target_os = "windows"))]
        {
            assert!(
                path.ends_with(".sagent"),
                "on Unix-like systems, should end with '.sagent', got: {path:?}"
            );
        }
        #[cfg(target_os = "windows")]
        {
            let lower = path.to_string_lossy().to_lowercase();
            assert!(
                lower.contains("sagent"),
                "on Windows, should contain 'sagent', got: {path:?}"
            );
        }
    }

    #[test]
    fn test_platform_default_sagent_home_is_under_user_home() {
        let home = get_user_home();
        let default_path = get_platform_default_sagent_home();

        #[cfg(not(target_os = "windows"))]
        assert!(
            default_path.starts_with(&home),
            "default path {default_path:?} should be under home {home:?}"
        );
    }

    // ── get_sagent_home_override ──────────────────────────────────────

    #[test]
    fn test_override_returns_none_outside_scope() {
        let result = get_sagent_home_override();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_override_returns_set_value_inside_scope() {
        let custom = "/tmp/sagent-test-override";
        SAGENT_HOME_OVERRIDE
            .scope(custom.to_string(), async {
                let result = get_sagent_home_override();
                assert_eq!(result, Some(PathBuf::from("/tmp/sagent-test-override")));
            })
            .await;
    }

    #[tokio::test]
    async fn test_override_isolated_between_scopes() {
        // First scope
        SAGENT_HOME_OVERRIDE
            .scope("/first/path".to_string(), async {
                assert_eq!(
                    get_sagent_home_override(),
                    Some(PathBuf::from("/first/path"))
                );
            })
            .await;

        // After first scope completes, override should be None again
        assert!(get_sagent_home_override().is_none());

        // Second scope with different value
        SAGENT_HOME_OVERRIDE
            .scope("/second/path".to_string(), async {
                assert_eq!(
                    get_sagent_home_override(),
                    Some(PathBuf::from("/second/path"))
                );
            })
            .await;

        // Back to None
        assert!(get_sagent_home_override().is_none());
    }

    // ── get_sagent_home ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_get_sagent_home_uses_override_when_set() {
        let custom = "/tmp/custom-sagent";
        SAGENT_HOME_OVERRIDE
            .scope(custom.to_string(), async {
                let home = get_sagent_home();
                assert_eq!(home, PathBuf::from("/tmp/custom-sagent"));
            })
            .await;
    }

    #[test]
    fn test_get_sagent_home_falls_back_to_platform_default() {
        // Without override, should return the platform default path
        let expected = get_platform_default_sagent_home();
        let actual = get_sagent_home();
        assert_eq!(expected, actual);
    }

    #[tokio::test]
    async fn test_get_sagent_home_override_prefix_not_confused_with_suffix() {
        // Ensure the override path is used verbatim, not appended to default
        SAGENT_HOME_OVERRIDE
            .scope("/opt/sagent-custom".to_string(), async {
                let home = get_sagent_home();
                assert_eq!(home, PathBuf::from("/opt/sagent-custom"));
                // Should NOT contain ".sagent"
                assert!(
                    !home.ends_with(".sagent"),
                    "override path should not be modified to append '.sagent'"
                );
            })
            .await;
    }

    // ── Integration: task_local + AtomicBool interaction ──────────────

    #[tokio::test]
    async fn test_get_sagent_home_prefers_override_over_fallback() {
        // Even after PROFILE_FALLBACK_WARNED has been toggled by earlier tests,
        // the override should still take priority.
        let custom = "/custom/override";
        SAGENT_HOME_OVERRIDE
            .scope(custom.to_string(), async {
                assert_eq!(get_sagent_home(), PathBuf::from("/custom/override"));
            })
            .await;
    }

    // ── Helper assertions for correctness ─────────────────────────────

    #[test]
    fn test_all_returned_paths_are_absolute() {
        assert!(get_sagent_home().is_absolute());
        assert!(get_platform_default_sagent_home().is_absolute());
    }
}