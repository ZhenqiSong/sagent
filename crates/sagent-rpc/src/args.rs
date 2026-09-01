//! `sagent-rpc` 启动参数定义。

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use sagent_config::{ProfileName, normalize_profile_name};

/// RPC 进程启动时固定的作用域参数。
#[derive(Debug, Parser)]
#[command(name = "sagent-rpc", about = "Sagent 本地只读 JSON-RPC 服务")]
pub struct RpcArgs {
    /// Sagent 根目录；省略时使用现有默认路径解析规则。
    #[arg(long)]
    pub home: Option<PathBuf>,
    /// 具名 Profile；省略时读取 active-profile。
    #[arg(long, value_parser = parse_profile)]
    pub profile: Option<ProfileName>,
}

/// 将命令行 Profile 字符串转换为经过路径安全校验的名称。
fn parse_profile(value: &str) -> Result<ProfileName> {
    normalize_profile_name(value)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::RpcArgs;

    #[test]
    fn parses_home_and_normalizes_profile() {
        let args = RpcArgs::try_parse_from([
            "sagent-rpc",
            "--home",
            r"D:\data\sagent",
            "--profile",
            " Coder ",
        ])
        .expect("合法参数应能解析");

        assert_eq!(
            args.home.as_deref(),
            Some(std::path::Path::new(r"D:\data\sagent"))
        );
        assert_eq!(
            args.profile.as_ref().map(|profile| profile.as_str()),
            Some("coder")
        );
    }

    #[test]
    fn optional_scope_arguments_are_allowed() {
        let args = RpcArgs::try_parse_from(["sagent-rpc"]).expect("省略可选参数应能解析");

        assert!(args.home.is_none());
        assert!(args.profile.is_none());
    }

    #[test]
    fn unsafe_profile_is_rejected_by_clap_value_parser() {
        let error = RpcArgs::try_parse_from(["sagent-rpc", "--profile", "../escape"])
            .expect_err("不安全 Profile 必须失败");

        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }
}
