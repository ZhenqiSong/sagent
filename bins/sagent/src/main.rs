//! sagent CLI 入口。
//!
//! 提供 stdio JSON-RPC、协议管理、健康检查和基础 Session CLI 子命令。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 CLI 入口占位
//! @change   2025-08-07 增强：Phase 0 Step 7 实现 rpc stdio 子命令
//! @change   2025-08-12 增强：Phase 0 Step 9 结构化日志、request_id span、BrokenPipe 日志
//! @change   2025-08-12 增强：Phase 0 Step 10 schema 生成命令

mod bootstrap;
mod cli;
mod dispatcher;
mod server;
mod stdio;

use clap::{CommandFactory, Parser, Subcommand};
use tracing::info;

/// Sagent — 模块化的本地优先 AI Agent Runtime
#[derive(Parser)]
#[command(name = "sagent")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "模块化的本地优先 AI Agent Runtime", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// JSON-RPC 子命令
    Rpc {
        #[command(subcommand)]
        mode: RpcMode,
    },
    /// 协议管理子命令
    Protocol {
        #[command(subcommand)]
        action: ProtocolAction,
    },
    /// Session 管理子命令
    Session {
        #[command(subcommand)]
        action: cli::SessionAction,
    },
    /// 检查 Runtime 和数据库是否可用dsa
    Health {
        /// 输出 JSON。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RpcMode {
    /// 启动 stdio JSON-RPC server（newline-delimited JSON）
    Stdio,
}

#[derive(Subcommand)]
enum ProtocolAction {
    /// 生成 JSON Schema 文件到 protocols/schemas/ 目录
    GenerateSchemas,
    /// 输出协议版本和 capabilities。
    Describe {
        /// 输出 JSON。
        #[arg(long)]
        json: bool,
    },
}

fn main() {
    // 在启动最早期注入 Sagent 环境标识（仅未设置时写入）
    bootstrap::advertise_sagent_env();

    // TODO：设置windows的 utf-8, 如有必要

    // 初始化日志（stderr），幂等调用
    sagent_api::logging::init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Rpc { mode }) => match mode {
            RpcMode::Stdio => server::run_stdio_server(),
        },
        Some(Commands::Protocol { action }) => match action {
            ProtocolAction::GenerateSchemas => generate_schemas(),
            ProtocolAction::Describe { json } => exit_on_error(cli::run_protocol_describe(json)),
        },
        Some(Commands::Session { action }) => exit_on_error(cli::run_session(action)),
        Some(Commands::Health { json }) => exit_on_error(cli::run_health(json)),
        // 无子命令时打印帮助信息
        None => {
            if let Err(error) = Cli::command().print_help() {
                eprintln!("错误: {error}");
                std::process::exit(1);
            }
            println!();
        },
    }
}

fn exit_on_error(result: Result<(), cli::CliError>) {
    if let Err(error) = result {
        eprintln!("错误: {error}");
        std::process::exit(1);
    }
}

/// 生成所有 JSON Schema 文件到 protocols/schemas/ 目录。
///
/// 从 Rust 代码中的 schema 定义生成静态 JSON 文件，
/// 确保 Rust 类型与协议 schema 始终保持一致。
/// CI 中运行 `git diff --exit-code -- protocols/schemas` 可检测不一致。
fn generate_schemas() {
    let schemas = sagent_api::schema::all_schemas();

    // 从 workspace 根目录定位 protocols/schemas/
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("无法定位 workspace 根目录");
    let schemas_dir = workspace_root.join("protocols").join("schemas");

    // 确保目录存在
    std::fs::create_dir_all(&schemas_dir).expect("无法创建 protocols/schemas 目录");

    for (filename, schema) in &schemas {
        let file_path = schemas_dir.join(filename);
        let json_str = serde_json::to_string_pretty(schema).expect("schema 序列化失败");
        std::fs::write(&file_path, format!("{}\n", json_str))
            .unwrap_or_else(|e| panic!("写入 {} 失败: {}", file_path.display(), e));
        info!("生成 schema: {}", file_path.display());
    }

    // 输出提示信息到 stdout（这是无状态命令，stdout 不用于 JSON-RPC）
    println!(
        "已生成 {} 个 schema 文件到 {}",
        schemas.len(),
        schemas_dir.display()
    );
}
