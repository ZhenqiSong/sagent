//! `sagent-rpc` 本地只读 JSON-RPC 进程入口。

mod args;
mod connection;
mod runtime_bootstrap;
mod service;
mod stdio;

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

/// 解析作用域、构造 Profile 隔离的 Runtime，并启动 NDJSON 请求循环。
async fn run() -> Result<()> {
    let args = args::RpcArgs::parse();
    let paths = match args.profile.as_ref() {
        Some(profile) => resolve_paths(args.home.as_deref(), Some(profile))?,
        None => resolve_active_paths(args.home.as_deref(), None)?,
    };
    let service = runtime_bootstrap::RuntimeBootstrap::from_paths(paths)?.into_service();
    let reader = tokio::io::BufReader::new(tokio::io::stdin());
    let writer = tokio::io::BufWriter::new(tokio::io::stdout());
    let connection = connection::ConnectionState::new();
    stdio::run(reader, writer, service, connection)
        .await
        .context("stdio RPC 循环失败")?;
    Ok(())
}
