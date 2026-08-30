//! Sagent 命令行入口。
//!
//! 作者：SongZQ

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use sagent_config::{list_profile_names, paths::platform_default_home, paths::profile_root};

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
    Ok(list_profile_names(&root)?
        .into_iter()
        .map(|profile| profile.as_str().to_owned())
        .collect())
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

    use super::{Cli, profile_list_lines, profile_list_root};

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
            vec!["default", "coder", "writer"]
        );
        fs::remove_dir_all(root).expect("应能清理测试目录");
    }

    #[test]
    fn profile_list_rejects_relative_home() {
        let error = profile_list_root(Some(std::path::Path::new("relative")))
            .expect_err("必须拒绝相对路径");

        assert!(error.to_string().contains("绝对路径"));
    }
}
