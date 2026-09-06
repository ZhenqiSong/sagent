//! `sagent-rpc` 本地只读 JSON-RPC 进程入口。

mod args;
mod connection;
mod stdio;

use std::io::{self, BufReader};

use anyhow::{Context, Result};
use clap::Parser;
use sagent_config::{resolve_active_paths, resolve_paths};
use sagent_protocol::{GatewayPingResult, GatewayService, SessionReadService, SessionService};
use sagent_store::Store;

/// 将只读协议服务适配到 Store；transport 不直接接触数据库。
struct RpcService {
    sessions: SessionService,
}

impl GatewayService for RpcService {
    fn ping(&self) -> GatewayPingResult {
        GatewayPingResult {
            ok: true,
            protocol_version: sagent_protocol::PROTOCOL_VERSION,
        }
    }
}

impl SessionReadService for RpcService {
    fn list_sessions(
        &self,
        params: &sagent_protocol::SessionListParams,
    ) -> Result<sagent_protocol::SessionListResult, sagent_protocol::ProtocolError> {
        self.sessions.list_sessions(params)
    }

    fn resume_session(
        &self,
        params: &sagent_protocol::SessionResumeParams,
    ) -> Result<sagent_protocol::SessionResumeResult, sagent_protocol::ProtocolError> {
        self.sessions.resume_session(params)
    }
}

/// 进程入口只负责把启动错误写到 stderr，避免污染 stdout 协议流。
fn main() {
    if let Err(error) = run() {
        eprintln!("sagent-rpc: {error:#}");
        std::process::exit(1);
    }
}

/// 解析作用域、以只读模式打开数据库，并启动 NDJSON 请求循环。
fn run() -> Result<()> {
    let args = args::RpcArgs::parse();
    let paths = match args.profile.as_ref() {
        Some(profile) => resolve_paths(args.home.as_deref(), Some(profile))?,
        None => resolve_active_paths(args.home.as_deref(), None)?,
    };
    let store = Store::open_readonly(&paths.state_db)
        .with_context(|| format!("无法打开 RPC 只读数据库：{}", paths.state_db.display()))?;
    store
        .verify_connection()
        .context("RPC 数据库连接检查失败")?;
    let service = RpcService {
        sessions: SessionService::new(store),
    };
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let mut connection = connection::ConnectionState::new();
    stdio::run(&mut reader, &mut stdout, &service, &mut connection)
        .context("stdio RPC 循环失败")?;
    Ok(())
}
