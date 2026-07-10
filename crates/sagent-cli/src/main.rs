//! sagent CLI — 个人智能 Agent 命令行工具。
//!
//! 入口文件，负责解析命令行参数、初始化日志系统，并分发到对应子命令。

use clap::{Parser, Subcommand};

/// sagent — 个人智能 Agent
#[derive(Parser)]
#[command(name = "sagent", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// 日志级别 (trace, debug, info, warn, error)
    #[arg(short, long, env = "SAGENT_LOG", default_value = "info")]
    log_level: String,
}

#[derive(Subcommand)]
enum Commands {
    /// 启动交互式对话（默认模式）
    Run {
        /// 要使用的 Profile 名称
        #[arg(short, long, env = "SAGENT_PROFILE")]
        profile: Option<String>,

        /// 单次对话消息（不提供则进入 REPL 交互模式）
        message: Vec<String>,
    },

    /// 启动多平台消息网关
    Gateway {
        /// 要连接的平台 (telegram, discord, slack)
        #[arg(short, long, value_delimiter = ',')]
        platforms: Vec<String>,
    },

    /// 列出/管理已注册工具
    Tools {
        /// 以列表形式展示所有工具
        #[arg(short, long)]
        list: bool,
    },

    /// 初始化配置向导
    Setup,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    sagent_core::logging::init_with_default(&cli.log_level);

    match cli.command {
        Commands::Run { profile, message } => {
            tracing::info!(?profile, ?message, "启动对话模式");
            sagent_cli::commands::run::execute(profile, message).await
        }
        Commands::Gateway { platforms } => {
            tracing::info!(?platforms, "启动 Gateway");
            tracing::warn!("Gateway 命令尚未实现");
            Ok(())
        }
        Commands::Tools { list } => {
            tracing::info!(list, "列出工具");
            tracing::warn!("Tools 命令尚未实现");
            Ok(())
        }
        Commands::Setup => {
            tracing::info!("启动配置向导");
            tracing::warn!("Setup 命令尚未实现");
            Ok(())
        }
    }
}
