//! Sagent Home 与 Profile 路径解析。
//!
//! 作者：SongZQ
//! 创建日期：2026-08-29

use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};

use crate::{ProfileName, read_active_profile};

/// 一个已解析的 Sagent 数据目录及其第一阶段需要访问的文件路径。
///
/// 该结构只描述路径，不创建目录或文件，也不打开数据库。
#[derive(Debug)]
pub struct SagentPaths {
    pub sagent_home: PathBuf,
    pub state_db: PathBuf,
    pub config_yaml: PathBuf,
}

pub fn resolve_paths(
    home_override: Option<&Path>,
    profile: Option<&ProfileName>,
) -> Result<SagentPaths> {
    // 显式命令行参数优先，随后才是进程环境；这让测试和嵌入式调用无需修改环境变量。
    let root = match home_override {
        Some(path) => path.to_path_buf(),
        None => std::env::var_os("SAGENT_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(platform_default_home),
    };

    if !root.is_absolute() {
        // 相对路径会随工作目录变化，且可能把状态写入意外位置，因此在边界处拒绝。
        bail!("SAGENT_HOME 必须是绝对路径");
    }

    let sagent_home = match profile {
        None => root,
        Some(profile) if profile.as_str() == "default" => profile_root(&root),
        Some(profile) => {
            // 命名 Profile 始终是根目录下的直接子目录，不能由 profile 名称逃逸此边界。
            let profile_dir = profile_root(&root).join("profiles").join(profile.as_str());

            if !profile_dir.is_dir() {
                bail!("profile 目录'{}'不存在", profile.as_str());
            }

            profile_dir
        }
    };

    Ok(SagentPaths {
        state_db: sagent_home.join("state.db"),
        config_yaml: sagent_home.join("config.yaml"),
        sagent_home,
    })
}

/// 解析当前应使用的 profile 路径。
///
/// profile_override 用于显式命令行选择，优先级高于根目录中的 active-profile；
/// 未提供覆盖项时，缺少选择文件会自然回退到 default。
pub fn resolve_active_paths(
    home_override: Option<&Path>,
    profile_override: Option<&ProfileName>,
) -> Result<SagentPaths> {
    let root = match home_override {
        Some(path) => path.to_path_buf(),
        None => std::env::var_os("SAGENT_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(platform_default_home),
    };
    if !root.is_absolute() {
        bail!("SAGENT_HOME 必须是绝对路径");
    }

    let root = profile_root(&root);
    let active_profile;
    let profile = match profile_override {
        Some(profile) => profile,
        None => {
            active_profile = read_active_profile(&root)?;
            &active_profile
        }
    };
    resolve_paths(Some(&root), Some(profile))
}

/// 返回当前平台默认的 Sagent 数据目录。
///
/// Windows: %LOCALAPPDATA%\sagent
/// POSIX:   ~/.sagent
///
/// 当系统未提供用户目录环境变量时，退回当前工作目录。该退回路径仍保持绝对形式，
/// 以满足 `resolve_paths()` 的安全约束。
pub fn platform_default_home() -> PathBuf {
    if cfg!(windows) {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                env::var_os("USERPROFILE")
                    .map(PathBuf::from)
                    .map(|home| home.join("AppData").join("Local"))
                    .filter(|path| path.is_absolute())
            })
            .unwrap_or_else(|| {
                env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join("AppData")
                    .join("Local")
            });
        return base.join("sagent");
    }

    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    home.join(".sagent")
}

pub fn profile_root(home: &Path) -> PathBuf {
    // `SAGENT_HOME` 可能已经指向 `<root>/profiles/<name>`。此时 profile 操作仍必须
    // 回到 `<root>`，否则会错误计算成 `<root>/profiles/<name>/profiles/<other>`。
    match (home.parent(), home.parent().and_then(Path::parent)) {
        (Some(parent), Some(root)) if parent.file_name().is_some_and(|name| name == "profiles") => {
            root.to_path_buf()
        }
        _ => home.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::{platform_default_home, profile_root, resolve_active_paths, resolve_paths};
    use crate::{normalize_profile_name, set_active_profile};

    #[test]
    fn profile_home_resolves_to_root() {
        assert_eq!(
            profile_root(Path::new("/opt/sagent/profiles/coder")),
            Path::new("/opt/sagent")
        );
    }

    #[test]
    fn default_home_is_its_own_root() {
        assert_eq!(
            profile_root(Path::new("/opt/sagent")),
            Path::new("/opt/sagent")
        );
    }

    #[test]
    fn shallow_path_is_unchanged() {
        assert_eq!(profile_root(Path::new("sagent")), Path::new("sagent"));
    }

    #[test]
    fn platform_default_home_is_absolute_and_uses_sagent_leaf() {
        let home = platform_default_home();

        assert!(home.is_absolute());
        assert_eq!(
            home.file_name().and_then(|name| name.to_str()),
            Some("sagent")
        );
    }

    #[test]
    fn resolve_paths_uses_explicit_default_profile_root() {
        let root =
            std::env::temp_dir().join(format!("sagent-config-default-{}", std::process::id()));
        fs::create_dir_all(&root).expect("应能创建测试目录");

        let default = normalize_profile_name("default").expect("default profile 应合法");
        let paths = resolve_paths(Some(&root), Some(&default)).expect("应能解析 default profile");

        assert_eq!(paths.sagent_home, root);
        assert_eq!(paths.state_db, paths.sagent_home.join("state.db"));
        assert_eq!(paths.config_yaml, paths.sagent_home.join("config.yaml"));

        fs::remove_dir_all(paths.sagent_home).expect("应能清理测试目录");
    }

    #[test]
    fn resolve_paths_uses_existing_named_profile() {
        let root =
            std::env::temp_dir().join(format!("sagent-config-profile-{}", std::process::id()));
        let expected_home = root.join("profiles").join("coder");
        fs::create_dir_all(&expected_home).expect("应能创建 profile 测试目录");

        let profile = normalize_profile_name("Coder").expect("profile 应合法");
        let paths = resolve_paths(Some(&root), Some(&profile)).expect("应能解析已存在的 profile");

        assert_eq!(paths.sagent_home, expected_home);

        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn resolve_paths_rejects_missing_named_profile() {
        let root =
            std::env::temp_dir().join(format!("sagent-config-missing-{}", std::process::id()));
        fs::create_dir_all(&root).expect("应能创建测试目录");
        let profile = normalize_profile_name("missing").expect("profile 应合法");

        let error =
            resolve_paths(Some(&root), Some(&profile)).expect_err("不存在的 profile 必须报错");

        assert!(error.to_string().contains("不存在"));
        assert!(!root.join("profiles").join("missing").exists());

        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn resolve_paths_rejects_relative_home_override() {
        let error =
            resolve_paths(Some(Path::new("relative-home")), None).expect_err("相对路径必须报错");

        assert!(error.to_string().contains("绝对路径"));
    }

    #[test]
    fn active_path_resolution_uses_saved_profile_unless_explicitly_overridden() {
        let root =
            std::env::temp_dir().join(format!("sagent-config-active-{}", std::process::id()));
        let coder_home = root.join("profiles").join("coder");
        fs::create_dir_all(&coder_home).expect("应能创建命名 profile");
        let coder = normalize_profile_name("coder").expect("名称应合法");
        set_active_profile(&root, &coder).expect("应能选择 coder");

        let active = resolve_active_paths(Some(&root), None).expect("应能解析保存的当前 profile");
        assert_eq!(active.sagent_home, coder_home);

        let default = normalize_profile_name("default").expect("名称应合法");
        let explicit =
            resolve_active_paths(Some(&root), Some(&default)).expect("显式 default 应覆盖当前选择");
        assert_eq!(explicit.sagent_home, root);

        fs::remove_dir_all(root).expect("应能清理测试目录");
    }
}
