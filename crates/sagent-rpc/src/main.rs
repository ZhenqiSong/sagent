//! `sagent-rpc` 本地只读 JSON-RPC 进程入口。

mod args;
// transport 实现放在同一目录，避免 stdio/WebSocket 扩展时把连接状态、输出桥接和
// 启动装配混在入口根目录；模块名保持稳定，调用方无需感知物理重组。
#[path = "transport/connection.rs"]
mod connection;
#[path = "transport/event_bridge.rs"]
mod event_bridge;
#[path = "bootstrap/runtime.rs"]
mod runtime_bootstrap;
mod service;
#[path = "transport/stdio.rs"]
mod stdio;
#[path = "transport/websocket.rs"]
mod websocket;

use anyhow::{Context, Result};
use clap::Parser;
use sagent_config::{resolve_active_paths, resolve_paths};

/// 进程入口只负责把启动错误写到 stderr，避免污染 stdout 协议流。
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("sagent-rpc: {error:#}");
        std::process::exit(1);
    }
}

/// 解析作用域、构造 Profile 隔离的 Runtime，并按显式参数启动本地 transport。
async fn run() -> Result<()> {
    let args = args::RpcArgs::parse();
    let paths = match args.profile.as_ref() {
        Some(profile) => resolve_paths(args.home.as_deref(), Some(profile))?,
        None => resolve_active_paths(args.home.as_deref(), None)?,
    };
    let bootstrap = runtime_bootstrap::RuntimeBootstrap::from_paths(paths)?;
    if let Some(address) = args.websocket_addr {
        websocket::run(address, std::sync::Arc::new(bootstrap))
            .await
            .context("WebSocket RPC 服务失败")?;
    } else {
        let service = bootstrap.open_service()?;
        let reader = tokio::io::BufReader::new(tokio::io::stdin());
        let writer = tokio::io::BufWriter::new(tokio::io::stdout());
        let connection = connection::ConnectionState::new();
        stdio::run(reader, writer, service, connection)
            .await
            .context("stdio RPC 循环失败")?;
    }
    Ok(())
}
