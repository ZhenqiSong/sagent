//! Sagent 命令行入口。
//!
//! 此文件只负责参数解析和命令路由；具体业务逻辑位于 `commands`，输出策略位于
//! `output`，以便新增命令时不会继续膨胀入口文件。
//!
//! 作者：SongZQ

mod commands;
mod output;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use crate::{commands::Command, output::OutputFormat};

#[derive(Debug, Parser)]
#[command(name = "sagent", version, about = "Sagent 命令行工具")]
struct Cli {
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    #[arg(long, global = true)]
    profile: Option<String>,
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    #[command(subcommand)]
    command: Command,
}

fn run(cli: Cli) -> Result<()> {
    let Cli {
        home,
        profile,
        format,
        command,
    } = cli;

    command.execute(home.as_deref(), profile.as_deref(), format)
}

fn main() -> Result<()> {
    run(Cli::parse())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Cli;

    #[test]
    fn parses_profile_list_with_absolute_home() {
        let cli = Cli::try_parse_from(["sagent", "--home", "C:\\sagent-test", "profile", "list"])
            .expect("profile list 参数应可解析");

        assert_eq!(
            cli.home.expect("应保留 home 参数"),
            std::path::PathBuf::from("C:\\sagent-test")
        );
    }
}
