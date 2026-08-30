//! Sagent 命令行入口。
//!
//! 作者：SongZQ

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sagent_config::{
    list_profile_names, normalize_profile_name, paths::platform_default_home, paths::profile_root,
    read_active_profile, set_active_profile,
};
use sagent_store::Store;

/// Sagent 命令行参数。
#[derive(Debug, Parser)]
#[command(name = "sagent", version, about = "Sagent 命令行工具")]
struct Cli {
    /// 覆盖 Sagent 根目录；必须是绝对路径，主要用于隔离部署和测试。
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

/// 一级命令。
#[derive(Debug, Subcommand)]
enum Command {
    /// 管理独立的 Sagent profile。
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
}

/// profile 子命令。
#[derive(Debug, Subcommand)]
enum ProfileCommand {
    /// 列出默认 profile 与根目录中所有可用的命名 profile。
    List,
    /// 创建一个拥有独立配置和会话数据库的命名 profile。
    Create {
        /// profile 名称；会规范化为小写。
        name: String,
    },
    /// 选择后续命令默认使用的 profile。
    Use {
        /// 已存在的 profile 名称；default 表示根目录 profile。
        name: String,
    },
}

/// 解析 profile list 所使用的根目录。
///
/// 当 home 恰好指向某个命名 profile 时，仍回到其父根目录列出全部 profile。
fn profile_list_root(home: Option<&Path>) -> Result<PathBuf> {
    let home = home
        .map(Path::to_path_buf)
        .unwrap_or_else(platform_default_home);
    if !home.is_absolute() {
        anyhow::bail!("--home 必须是绝对路径");
    }
    Ok(profile_root(&home))
}

/// 返回 profile list 的稳定文本行，便于 CLI 输出与单元测试复用。
fn profile_list_lines(home: Option<&Path>) -> Result<Vec<String>> {
    let root = profile_list_root(home)?;
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

/// 一个新 profile 的最小配置文件。
///
/// 此处只建立合法的空 YAML 文档；具体模型、Provider 等用户配置会在后续
/// setup 命令中写入，避免 create 命令猜测用户的运行偏好。
const INITIAL_CONFIG_YAML: &str = "# Sagent profile configuration.\n{}\n";

/// 创建命名 profile 的目录、初始配置和 SQLite 状态库。
///
/// 创建只允许落在根目录的 profiles 直接子目录中。初始化失败时会删除本次刚
/// 创建的 profile 目录，避免留下可被 list 误认为完整 profile 的半成品。
fn create_profile(home: Option<&Path>, name: &str) -> Result<PathBuf> {
    let root = profile_list_root(home)?;
    let profile = normalize_profile_name(name)?;
    if profile.as_str() == "default" {
        anyhow::bail!("default profile 使用根目录，无需创建");
    }

    create_profile_with_initializer(&root, profile.as_str(), |profile_dir| {
        fs::write(profile_dir.join("config.yaml"), INITIAL_CONFIG_YAML)
            .context("写入初始 config.yaml 失败")?;
        Store::open_readwrite(&profile_dir.join("state.db"))
            .context("初始化 profile state.db 失败")?;
        Ok(())
    })
}

/// 执行创建目录和失败回滚；初始化器抽出后可精确测试半成品清理行为。
fn create_profile_with_initializer(
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
fn use_profile(home: Option<&Path>, name: &str) -> Result<String> {
    let root = profile_list_root(home)?;
    let profile = normalize_profile_name(name)?;
    set_active_profile(&root, &profile)?;
    Ok(profile.as_str().to_owned())
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Profile {
            command: ProfileCommand::List,
        } => {
            for line in profile_list_lines(cli.home.as_deref())? {
                println!("{line}");
            }
        }
        Command::Profile {
            command: ProfileCommand::Create { name },
        } => {
            let path = create_profile(cli.home.as_deref(), &name)?;
            println!("已创建 profile: {}", path.display());
        }
        Command::Profile {
            command: ProfileCommand::Use { name },
        } => {
            let profile = use_profile(cli.home.as_deref(), &name)?;
            println!("当前 profile: {profile}");
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    run(Cli::parse())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use clap::Parser;
    use sagent_store::Store;

    use super::{
        Cli, create_profile, create_profile_with_initializer, profile_list_lines,
        profile_list_root, use_profile,
    };

    #[test]
    fn parses_profile_list_with_absolute_home() {
        let cli = Cli::try_parse_from(["sagent", "--home", "C:\\\\sagent-test", "profile", "list"])
            .expect("profile list 参数应可解析");

        assert_eq!(
            cli.home.expect("应保留 home 参数"),
            std::path::PathBuf::from("C:\\\\sagent-test")
        );
    }

    #[test]
    fn profile_list_returns_default_then_sorted_named_profiles() {
        let root = std::env::temp_dir().join(format!("sagent-cli-profiles-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("profiles").join("writer")).expect("应能创建 writer profile");
        fs::create_dir_all(root.join("profiles").join("coder")).expect("应能创建 coder profile");

        assert_eq!(
            profile_list_lines(Some(&root)).expect("应能列出 profile"),
            vec!["* default", "  coder", "  writer"]
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn profile_list_rejects_relative_home() {
        let error = profile_list_root(Some(std::path::Path::new("relative")))
            .expect_err("必须拒绝相对路径");

        assert!(error.to_string().contains("绝对路径"));
    }

    #[test]
    fn create_profile_writes_config_and_migrated_database() {
        let root = std::env::temp_dir().join(format!("sagent-cli-create-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");

        let path = create_profile(Some(&root), "Coder").expect("应能创建 profile");

        assert_eq!(path, root.join("profiles").join("coder"));
        assert_eq!(
            fs::read_to_string(path.join("config.yaml")).expect("应能读取初始配置"),
            super::INITIAL_CONFIG_YAML
        );
        assert!(
            Store::open_readonly(&path.join("state.db")).is_ok(),
            "创建命令应完成数据库迁移"
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn create_profile_rejects_existing_directory_without_overwriting_it() {
        let root =
            std::env::temp_dir().join(format!("sagent-cli-duplicate-{}", std::process::id()));
        let profile_dir = root.join("profiles").join("coder");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&profile_dir).expect("应能创建既有 profile");
        fs::write(profile_dir.join("keep.txt"), "do not overwrite").expect("应能写入哨兵文件");

        assert!(create_profile(Some(&root), "coder").is_err());
        assert_eq!(
            fs::read_to_string(profile_dir.join("keep.txt")).expect("既有文件不应被删除"),
            "do not overwrite"
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn create_profile_removes_partial_directory_when_initialization_fails() {
        let root =
            std::env::temp_dir().join(format!("sagent-cli-create-fail-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("应能创建测试根目录");

        let result = create_profile_with_initializer(&root, "broken", |_| {
            anyhow::bail!("模拟数据库初始化失败")
        });

        assert!(result.is_err());
        assert!(
            !root.join("profiles").join("broken").exists(),
            "初始化失败后不能留下半成品 profile"
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn use_profile_changes_list_marker_and_rejects_unknown_name() {
        let root = std::env::temp_dir().join(format!("sagent-cli-use-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("profiles").join("coder")).expect("应能创建 profile");

        assert_eq!(
            use_profile(Some(&root), "Coder").expect("应能选择 profile"),
            "coder"
        );
        assert_eq!(
            profile_list_lines(Some(&root)).expect("应能标记当前 profile"),
            vec!["  default", "* coder"]
        );
        assert!(use_profile(Some(&root), "missing").is_err());
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }
}
