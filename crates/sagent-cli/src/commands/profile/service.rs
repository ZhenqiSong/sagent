//! Profile 创建生命周期服务。
//!
//! 本模块只负责 CLI 创建 Profile 时的目录、配置和存储初始化；Profile 名称、路径索引和
//! active-profile 状态由 `sagent_config::Profile` 负责，输出格式由命令 handler 负责。
//!
//! 作者：SongZQ

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use sagent_config::{normalize_profile_name, resolve_paths};

use crate::commands::storage::storage_from_paths;

/// 一个新 Profile 的最小配置文件。
const INITIAL_CONFIG_YAML: &str = "# Sagent profile configuration.\n{}\n";

/// Profile 创建生命周期服务。
pub struct ProfileService;

impl ProfileService {
    /// 创建命名 Profile 并初始化配置与默认存储。
    pub(crate) fn create(root: &Path, name: &str) -> Result<PathBuf> {
        let profile = normalize_profile_name(name)?;
        if profile.as_str() == "default" {
            anyhow::bail!("default profile 使用根目录，无需创建");
        }
        if !root.is_absolute() {
            anyhow::bail!("Sagent 根目录必须是绝对路径");
        }

        Self::create_with_initializer(root, profile.as_str(), |profile_dir| {
            fs::write(profile_dir.join("config.yaml"), INITIAL_CONFIG_YAML)
                .context("写入初始 config.yaml 失败")?;
            let paths =
                resolve_paths(Some(profile_dir), None).context("解析新 Profile 路径失败")?;
            storage_from_paths(&paths)
                .context("创建新 Profile 存储上下文失败")?
                .initialize()?;
            Ok(())
        })
    }

    /// 执行创建目录和失败回滚；初始化器抽出后可精确测试半成品清理行为。
    fn create_with_initializer(
        root: &Path,
        name: &str,
        initialize: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<PathBuf> {
        let profile = normalize_profile_name(name)?;
        if profile.as_str() == "default" {
            anyhow::bail!("default profile 使用根目录，无需创建");
        }
        if !root.is_absolute() {
            anyhow::bail!("Sagent 根目录必须是绝对路径");
        }

        let profile_dir = root.join("profiles").join(profile.as_str());
        let parent = profile_dir
            .parent()
            .expect("profile 目录始终位于 profiles 子目录中");
        fs::create_dir_all(parent)
            .with_context(|| format!("创建 profile 父目录失败：{}", parent.display()))?;
        fs::create_dir(&profile_dir).with_context(|| {
            format!("创建 profile '{}' 失败；名称可能已经存在", profile.as_str())
        })?;

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
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sagent_store::SqliteDatabase;

    use super::{INITIAL_CONFIG_YAML, ProfileService};

    fn test_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sagent-cli-profile-{name}-{}", std::process::id()))
    }

    #[test]
    fn create_initializes_files_and_removes_partial_directory_on_failure() {
        let root_dir = test_root("create");
        let _ = fs::remove_dir_all(&root_dir);
        fs::create_dir_all(&root_dir).expect("应能创建测试根目录");

        let path = ProfileService::create(&root_dir, "Coder").expect("应能创建 profile");
        assert_eq!(path, root_dir.join("profiles").join("coder"));
        assert_eq!(
            fs::read_to_string(path.join("config.yaml")).unwrap(),
            INITIAL_CONFIG_YAML
        );
        assert!(SqliteDatabase::open_readonly(&path.join("state.db")).is_ok());

        let result = ProfileService::create_with_initializer(&root_dir, "broken", |_| {
            anyhow::bail!("模拟失败")
        });
        assert!(result.is_err());
        assert!(!root_dir.join("profiles").join("broken").exists());
        fs::remove_dir_all(root_dir).expect("应能清理测试目录");
    }
}
