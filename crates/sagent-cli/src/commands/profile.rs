//! Profile 管理命令的业务实现。
//!
//! 作者：SongZQ

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::Subcommand;
use sagent_config::{
    list_profile_names, normalize_profile_name, paths::platform_default_home, paths::profile_root,
    read_active_profile, set_active_profile,
};
use sagent_store::Store;

use crate::output::{OutputFormat, print_output};

/// `profile` 分组下的命令参数与处理器。
#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    List,
    Create { name: String },
    Use { name: String },
}

impl ProfileCommand {
    /// 执行 profile 子命令并按用户选择的格式输出。
    pub fn execute(self, home: Option<&Path>, format: OutputFormat) -> Result<()> {
        match self {
            Self::List => {
                let lines = list_lines(home)?;
                print_output(format, &lines, lines.clone())
            }
            Self::Create { name } => {
                let path = create(home, &name)?.display().to_string();
                let value = serde_json::json!({ "path": path.clone() });
                print_output(format, &value, vec![format!("已创建 profile: {path}")])
            }
            Self::Use { name } => {
                let selected = select(home, &name)?;
                let value = serde_json::json!({ "profile": selected.clone() });
                print_output(format, &value, vec![format!("当前 profile: {selected}")])
            }
        }
    }
}

/// 一个新 profile 的最小配置文件。
const INITIAL_CONFIG_YAML: &str = "# Sagent profile configuration.\n{}\n";

/// 解析 profile 命令所使用的根目录。
pub fn root(home: Option<&Path>) -> Result<PathBuf> {
    let home = home
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("SAGENT_HOME").map(PathBuf::from))
        .unwrap_or_else(platform_default_home);
    if !home.is_absolute() {
        anyhow::bail!("--home 必须是绝对路径");
    }
    Ok(profile_root(&home))
}

/// 返回 profile list 的稳定文本行。
pub fn list_lines(home: Option<&Path>) -> Result<Vec<String>> {
    let root = root(home)?;
    let active = read_active_profile(&root)?;
    Ok(list_profile_names(&root)?
        .into_iter()
        .map(|profile| {
            if profile == active {
                format!("* {}", profile.as_str())
            } else {
                format!("  {}", profile.as_str())
            }
        })
        .collect())
}

/// 创建命名 profile 的目录、初始配置和 SQLite 状态库。
pub fn create(home: Option<&Path>, name: &str) -> Result<PathBuf> {
    let root = root(home)?;
    let profile = normalize_profile_name(name)?;
    if profile.as_str() == "default" {
        anyhow::bail!("default profile 使用根目录，无需创建");
    }

    create_with_initializer(&root, profile.as_str(), |profile_dir| {
        fs::write(profile_dir.join("config.yaml"), INITIAL_CONFIG_YAML)
            .context("写入初始 config.yaml 失败")?;
        Store::open_readwrite(&profile_dir.join("state.db"))
            .context("初始化 profile state.db 失败")?;
        Ok(())
    })
}

/// 执行创建目录和失败回滚；初始化器抽出后可精确测试半成品清理行为。
pub fn create_with_initializer(
    root: &Path,
    name: &str,
    initialize: impl FnOnce(&Path) -> Result<()>,
) -> Result<PathBuf> {
    let profile = normalize_profile_name(name)?;
    if profile.as_str() == "default" {
        anyhow::bail!("default profile 使用根目录，无需创建");
    }
    if !root.is_absolute() {
        anyhow::bail!("--home 必须是绝对路径");
    }

    let profile_dir = profile_root(root).join("profiles").join(profile.as_str());
    let parent = profile_dir
        .parent()
        .expect("profile 目录始终位于 profiles 子目录中");
    fs::create_dir_all(parent)
        .with_context(|| format!("创建 profile 父目录失败：{}", parent.display()))?;
    fs::create_dir(&profile_dir)
        .with_context(|| format!("创建 profile '{}' 失败；名称可能已经存在", profile.as_str()))?;

    if let Err(error) = initialize(&profile_dir) {
        fs::remove_dir_all(&profile_dir).with_context(|| {
            format!(
                "清理初始化失败的 profile 目录失败：{}",
                profile_dir.display()
            )
        })?;
        return Err(error);
    }
    Ok(profile_dir)
}

/// 选择一个 profile 并返回规范化后的名称。
pub fn select(home: Option<&Path>, name: &str) -> Result<String> {
    let root = root(home)?;
    let profile = normalize_profile_name(name)?;
    set_active_profile(&root, &profile)?;
    Ok(profile.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use sagent_store::Store;

    use super::{INITIAL_CONFIG_YAML, create, create_with_initializer, list_lines, root, select};

    fn test_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sagent-cli-profile-{name}-{}", std::process::id()))
    }

    #[test]
    fn lists_profiles_in_stable_order_and_marks_active_one() {
        let root_dir = test_root("list");
        let _ = fs::remove_dir_all(&root_dir);
        fs::create_dir_all(root_dir.join("profiles").join("writer")).expect("应能创建 writer");
        fs::create_dir_all(root_dir.join("profiles").join("coder")).expect("应能创建 coder");

        assert_eq!(
            list_lines(Some(&root_dir)).expect("应能列出 profile"),
            vec!["* default", "  coder", "  writer"]
        );
        select(Some(&root_dir), "coder").expect("应能选择 profile");
        assert_eq!(
            list_lines(Some(&root_dir)).expect("应能标记当前 profile"),
            vec!["  default", "* coder", "  writer"]
        );
        fs::remove_dir_all(root_dir).expect("应能清理测试目录");
    }

    #[test]
    fn create_initializes_files_and_removes_partial_directory_on_failure() {
        let root_dir = test_root("create");
        let _ = fs::remove_dir_all(&root_dir);
        fs::create_dir_all(&root_dir).expect("应能创建测试根目录");

        let path = create(Some(&root_dir), "Coder").expect("应能创建 profile");
        assert_eq!(path, root_dir.join("profiles").join("coder"));
        assert_eq!(
            fs::read_to_string(path.join("config.yaml")).unwrap(),
            INITIAL_CONFIG_YAML
        );
        assert!(Store::open_readonly(&path.join("state.db")).is_ok());

        let result = create_with_initializer(&root_dir, "broken", |_| anyhow::bail!("模拟失败"));
        assert!(result.is_err());
        assert!(!root_dir.join("profiles").join("broken").exists());
        fs::remove_dir_all(root_dir).expect("应能清理测试目录");
    }

    #[test]
    fn rejects_relative_home_and_does_not_overwrite_existing_profile() {
        assert!(root(Some(Path::new("relative"))).is_err());

        let root_dir = test_root("duplicate");
        let profile_dir = root_dir.join("profiles").join("coder");
        let _ = fs::remove_dir_all(&root_dir);
        fs::create_dir_all(&profile_dir).expect("应能创建既有 profile");
        fs::write(profile_dir.join("keep.txt"), "keep").expect("应能写入哨兵文件");
        assert!(create(Some(&root_dir), "coder").is_err());
        assert_eq!(
            fs::read_to_string(profile_dir.join("keep.txt")).unwrap(),
            "keep"
        );
        fs::remove_dir_all(root_dir).expect("应能清理测试目录");
    }
}
