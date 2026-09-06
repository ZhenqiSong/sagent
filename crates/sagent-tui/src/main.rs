//! `sagent-tui` 的最小启动入口。
//!
//! 当前步骤只建立参数和 reducer 边界，尚不进入 raw mode 或启动 RPC 子进程；这样终端
//! 生命周期、网络 I/O 与 ViewModel 以后可以分别演进，不会在入口文件互相耦合。

mod app;
mod args;

use anyhow::Result;
use clap::Parser;

use crate::{app::AppState, args::TuiArgs};

/// 运行骨架并保留将来交给 RPC client 的启动作用域。
fn run(args: TuiArgs) -> Result<()> {
    // 本阶段不解析 home/profile，更不读取 SQLite。原始值仅是未来启动 sagent-rpc 时
    // 的受控 argv 事实，不能成为 reducer 或 UI 事件携带的可变路径参数。
    let _scope = (args.home, args.profile);
    let _state = AppState::default();
    Ok(())
}

/// 入口只向 stderr 报告启动错误，未来 stdout 始终留给真正的 UI 终端渲染。
fn main() {
    if let Err(error) = run(TuiArgs::parse()) {
        eprintln!("sagent-tui: {error:#}");
        std::process::exit(1);
    }
}
