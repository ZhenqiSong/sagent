//! sagent CLI 入口。
//!
//! Phase 0 提供 `rpc stdio` 和 `protocol generate-schemas` 子命令。
//! 后续 Phase 将添加 `config`、`session` 等子命令。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 CLI 入口占位
//! @change   2025-08-07 增强：Phase 0 Step 7 实现 rpc stdio 子命令
//! @change   2025-08-12 增强：Phase 0 Step 9 结构化日志、request_id span、BrokenPipe 日志
//! @change   2025-08-12 增强：Phase 0 Step 10 schema 生成命令

mod dispatcher;
mod stdio;

use clap::{Parser, Subcommand};
use sagent_config::Config;
use sagent_runtime::Runtime;
use sagent_types::version::Capabilities;
use tracing::{error, info, warn};

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
    /// 协议管理子命令
    Protocol {
        #[command(subcommand)]
        action: ProtocolAction,
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
}

fn main() {
    // 初始化日志（stderr），幂等调用
    sagent_api::logging::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Rpc { mode } => match mode {
            RpcMode::Stdio => run_stdio_server(),
        },
        Commands::Protocol { action } => match action {
            ProtocolAction::GenerateSchemas => generate_schemas(),
        },
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
    let runtime = match Runtime::open(Config::default()) {
        Ok(runtime) => runtime,
        Err(error) => {
            error!(error = %error, "Runtime 初始化失败，拒绝接受 RPC");
            return;
        },
    };
    let caps = Capabilities::runtime_capabilities();
    let pv = caps.protocol_version();
    let async_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("创建 Tokio runtime 失败");
    let mut subscriptions = dispatcher::Subscriptions::new();

    // 启动日志：记录协议版本、runtime 版本和 capabilities
    info!(
        protocol = %pv.protocol,
        version = pv.version,
        runtime_version = %pv.runtime_version,
        features = ?pv.features,
        "sagent stdio JSON-RPC server 启动"
    );

    let mut reader = stdio::LineReader::new();
    let mut writer = stdio::LineWriter::new();

    loop {
        // 读取下一行
        let line = match reader.read_line() {
            Some(Ok(line)) => line,
            Some(Err(e)) if stdio::is_line_too_large(&e) => {
                error!(error = %e, error_code = -32003, "输入行超过协议限制，继续处理");
                let response = dispatcher::build_error_response(
                    None,
                    &sagent_api::error::ErrorObject::payload_too_large(
                        "request line exceeds 1048576 bytes",
                    ),
                );
                if let Err(write_error) = writer.write_value(&response) {
                    warn!(error = %write_error, "超长输入错误响应写入失败，退出");
                    break;
                }
                continue;
            },
            Some(Err(e)) => {
                error!(
                    error = %e,
                    error_kind = ?e.kind(),
                    "stdin 读取错误，退出"
                );
                break;
            },
            None => {
                info!("stdin EOF，正常退出");
                break;
            },
        };

        // 先发出已排队的 live event，再处理下一条请求。
        for event in dispatcher::drain_events(&mut subscriptions) {
            if let Err(error) = writer.write_value(&event) {
                warn!(error = %error, "事件写入失败，退出");
                break;
            }
        }

        // 分发处理
        let result = dispatcher::dispatch_runtime(
            &line,
            &caps,
            &runtime,
            &async_runtime,
            &mut subscriptions,
        );
        match result {
            Ok(Some(response)) => {
                // 成功响应 → 写 stdout
                if let Err(e) = writer.write_value(&response) {
                    error!(
                        error = %e,
                        error_kind = ?e.kind(),
                        "stdout 写入失败（BrokenPipe 或 peer 断开），退出"
                    );
                    break;
                }
            },
            Ok(None) => {
                // notification → 不写 response（日志已在 dispatcher 中记录）
            },
            Err((id, err_obj)) => {
                // 错误响应 → 写 stdout（日志已在 dispatcher 中记录）
                let error_response = dispatcher::build_error_response(id, &err_obj);
                if let Err(e) = writer.write_value(&error_response) {
                    // stdout 写失败（如 BrokenPipe），写 stderr 后退出
                    warn!(
                        error = %e,
                        error_kind = ?e.kind(),
                        error_code = err_obj.code,
                        "stdout 写入失败，无法返回错误响应，退出"
                    );
                    break;
                }
            },
        }
    }

    if let Err(error) = async_runtime.block_on(runtime.shutdown()) {
        warn!(error = %error, "Runtime shutdown 失败");
    }
    info!("sagent stdio JSON-RPC server 已停止");
}
