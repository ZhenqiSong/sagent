//! TUI 的启动参数。

use std::path::PathBuf;

use clap::Parser;

/// 传递给未来 RPC 子进程的固定启动作用域。
///
/// 参数不进入 JSON-RPC 请求；Profile 的真实路径解析由 `sagent-rpc` 在进程启动时完成，
/// 防止一个活跃 TUI 连接通过单条请求跨越 Profile 边界。
#[derive(Debug, Parser)]
#[command(name = "sagent-tui", version, about = "Sagent 终端交互客户端")]
pub struct TuiArgs {
    /// Sagent 根目录；后续作为 `sagent-rpc --home` 的受控参数。
    #[arg(long)]
    pub home: Option<PathBuf>,
    /// 具名 Profile；后续作为 `sagent-rpc --profile` 的受控参数。
    #[arg(long)]
    pub profile: Option<String>,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::TuiArgs;

    #[test]
    fn preserves_scope_only_for_future_rpc_spawn() {
        let args = TuiArgs::try_parse_from([
            "sagent-tui",
            "--home",
            r"D:\data\sagent",
            "--profile",
            "coder",
        ])
        .expect("合法启动参数应可解析");

        assert_eq!(args.home, Some(r"D:\data\sagent".into()));
        assert_eq!(args.profile.as_deref(), Some("coder"));
    }
}
