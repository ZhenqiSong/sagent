use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};

use crate::ProfileName;

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
    let root = match home_override {
        Some(path) => path.to_path_buf(),
        None => std::env::var_os("SAGENT_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(platform_default_home),
    };

    if !root.is_absolute() {
        bail!("SAGENT_HOME 必须是绝对路径");
    }

    let sagent_home = match profile {
        None => root,
        Some(profile) if profile.as_str() == "default" => profile_root(&root),
        Some(profile) => {
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

/// 返回当前平台默认的 Sagent 数据目录。
///
/// Windows: %LOCALAPPDATA%\sagent
/// POSIX:   ~/.sagent
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

    use super::{platform_default_home, profile_root, resolve_paths};
    use crate::normalize_profile_name;

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
}
