//! `sagent-tui` 的最小启动入口。
//!
//! 入口只组装受控 RPC 子进程、纯 AppState 与终端循环；具体协议 I/O 和 UI 渲染分别在
//! 独立模块中维护，避免随着会话功能增加而把入口变成调度器。

mod app;
mod args;
mod rpc;
mod terminal;
mod ui;

use std::time::Duration;

use anyhow::Result;
use clap::Parser;

use crate::{
    app::{AppAction, AppState, load_sessions, reduce, restore_active_session},
    args::TuiArgs,
    rpc::RpcClient,
};

const MAX_RECONNECT_DELAY_SECS: u64 = 30;

/// 运行 TUI，并在 transport 断开后以有界退避重建同一 Profile 的 RPC 子进程。
async fn run(args: TuiArgs) -> Result<()> {
    // home/profile 仅在 RpcClient::spawn 构造子进程 argv 时读取；请求 params 从不携带
    // 作用域，避免一个活跃连接借业务请求穿越 Profile 隔离边界。
    let mut state = AppState::default();
    let mut reconnect_attempt = 0;
    loop {
        reduce(&mut state, AppAction::RpcStarting);
        let mut client = connect(&args, &mut state).await?;
        if state.active_session.is_some() {
            restore_active_session(&client, &mut state).await?;
        } else {
            load_sessions(&client, &mut state).await?;
        }
        let terminal_exit = match terminal::run(state, &mut client).await {
            Ok(exit) => exit,
            Err(error) => {
                client.shutdown().await;
                return Err(error);
            }
        };
        match terminal_exit {
            terminal::TerminalExit::Quit => {
                client.shutdown().await;
                return Ok(());
            }
            terminal::TerminalExit::Reconnect(next_state) => {
                client.shutdown().await;
                state = *next_state;
                let delay = reconnect_delay(reconnect_attempt);
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// 启动并完成握手；失败时附加有界 stderr 摘要，且不让 stdout 协议内容泄漏到诊断。
async fn connect(args: &TuiArgs, state: &mut AppState) -> Result<RpcClient> {
    let mut client = RpcClient::spawn(args).await?;
    if let Err(error) = client.handshake(state).await {
        let stderr = client.stderr_summary().await;
        client.shutdown().await;
        return Err(if stderr.is_empty() {
            error
        } else {
            error.context(format!("RPC stderr 摘要：{stderr}"))
        });
    }
    Ok(client)
}

/// 指数退避始终有上限，避免持续启动失败时快速重生子进程占用资源。
fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_secs((1_u64 << attempt.min(5)).min(MAX_RECONNECT_DELAY_SECS))
}

/// 入口只向 stderr 报告启动错误，未来 stdout 始终留给真正的 UI 终端渲染。
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    if let Err(error) = run(TuiArgs::parse()).await {
        eprintln!("sagent-tui: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::reconnect_delay;

    #[test]
    fn reconnect_backoff_is_exponential_and_bounded() {
        assert_eq!(reconnect_delay(0), Duration::from_secs(1));
        assert_eq!(reconnect_delay(1), Duration::from_secs(2));
        assert_eq!(reconnect_delay(3), Duration::from_secs(8));
        assert_eq!(reconnect_delay(u32::MAX), Duration::from_secs(30));
    }
}
