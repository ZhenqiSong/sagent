//! sagent CLI 入口。
//!
//! Phase 0 提供 `rpc stdio` 子命令，运行最小 JSON-RPC echo server。
//! 后续 Phase 将添加 `protocol describe`、`config`、`session` 等子命令。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 CLI 入口占位
//! @change   2025-08-07 增强：Phase 0 Step 7 实现 rpc stdio 子命令

mod dispatcher;
mod stdio;

use clap::{Parser, Subcommand};
use sagent_types::version::Capabilities;
use std::io::Write;
use tracing::{debug, error, info};

/// Sagent — 模块化的本地优先 AI Agent Runtime
#[derive(Parser)]
#[command(name = "sagent")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "模块化的本地优先 AI Agent Runtime", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// JSON-RPC 子命令
    Rpc {
        #[command(subcommand)]
        mode: RpcMode,
    },
}

#[derive(Subcommand)]
enum RpcMode {
    /// 启动 stdio JSON-RPC server（newline-delimited JSON）
    Stdio,
}

fn main() {
    // 初始化日志（stderr），幂等调用
    sagent_api::logging::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Rpc { mode } => match mode {
            RpcMode::Stdio => run_stdio_server(),
        },
    }
}

/// 运行 stdio JSON-RPC server 的主循环。
///
/// 处理流程：
/// 1. 从 stdin 逐行读取 JSON-RPC request/notification
/// 2. 分发到 dispatcher 处理
/// 3. 将 response 写入 stdout（单行 JSON，立即 flush）
/// 4. 错误写 stderr 日志
/// 5. stdin EOF 时正常退出（返回码 0）
///
/// stdout 写失败或 BrokenPipe 时有序退出，不 panic。
fn run_stdio_server() {
    let caps = Capabilities::phase0_defaults();
    let pv = sagent_types::version::ProtocolVersion::default();

    // 启动日志记录协议版本和 capabilities
    info!(
        protocol = %pv.protocol,
        version = pv.version,
        runtime_version = %pv.runtime_version,
        features = ?pv.features,
        "sagent stdio JSON-RPC server started"
    );

    let mut reader = stdio::LineReader::new();
    let mut writer = stdio::LineWriter::new();

    loop {
        // 读取下一行
        let line = match reader.read_line() {
            Some(Ok(line)) => line,
            Some(Err(e)) => {
                error!(error = %e, "stdin 读取错误，退出");
                break;
            },
            None => {
                debug!("stdin EOF，正常退出");
                break;
            },
        };

        // 分发处理
        match dispatcher::dispatch(&line, &caps) {
            Ok(Some(response)) => {
                // 成功响应 → 写 stdout
                if let Err(e) = writer.write_value(&response) {
                    error!(error = %e, "stdout 写入失败，退出");
                    break;
                }
            },
            Ok(None) => {
                // notification → 不写 response
                debug!("notification processed, no response");
            },
            Err((id, err_obj)) => {
                // 错误响应 → 写 stdout
                let error_response = dispatcher::build_error_response(id, &err_obj);
                error!(
                    code = err_obj.code,
                    message = %err_obj.message,
                    "请求处理错误"
                );
                if let Err(e) = writer.write_value(&error_response) {
                    // stdout 写失败（如 BrokenPipe），走 stderr 后退出
                    let _ = writeln!(std::io::stderr(), "stdout write failed: {}", e);
                    break;
                }
            },
        }
    }

    info!("sagent stdio JSON-RPC server stopped");
}
