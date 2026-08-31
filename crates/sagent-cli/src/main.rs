//! Sagent 命令行入口。
//!
//! 此文件只负责参数解析和命令路由；具体业务逻辑位于 `commands`，输出策略位于
//! `output`，以便新增命令时不会继续膨胀入口文件。
//!
//! 作者：SongZQ

mod commands;
mod output;

use std::{path::PathBuf, process::ExitCode};

use anyhow::Result;
use clap::Parser;

use crate::{
    commands::{Command, CommandContext},
    output::OutputFormat,
};

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

    let context = CommandContext {
        home,
        profile,
        format,
    };
    command.execute(&context)
}

/// 将当前已知的 CLI 边界错误映射为稳定退出码。
///
/// 具体业务错误仍由下层保留完整诊断；此处只决定脚本调用方的错误类别。后续引入
/// `sagent-types` 的领域错误后，应改为基于类型而非文本分类。
fn exit_code(error: &anyhow::Error) -> ExitCode {
    let diagnostic = format!("{error:#}");
    let code = if diagnostic.contains("--home 必须是绝对路径")
        || diagnostic.contains("profile 名称")
        || diagnostic.contains("default profile")
        || diagnostic.contains("会话标题不能为空")
        || diagnostic.contains("会话标题不能超过")
        || diagnostic.contains("会话结束原因不能为空")
        || diagnostic.contains("消息 ID 必须是正整数")
        || diagnostic.contains("全文搜索词不能为空")
        || diagnostic.contains("全文搜索 limit")
    {
        2
    } else if diagnostic.contains("state.db 不存在") || diagnostic.contains("profile 不存在")
    {
        3
    } else if diagnostic.contains("数据库")
        || diagnostic.contains("SQLite")
        || diagnostic.contains("FTS5")
        || diagnostic.contains("schema")
    {
        4
    } else {
        1
    };
    ExitCode::from(code)
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let exit_code = error.exit_code();
            let _ = error.print();
            return ExitCode::from(exit_code as u8);
        }
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误：{error:#}");
            exit_code(&error)
        }
    }
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
