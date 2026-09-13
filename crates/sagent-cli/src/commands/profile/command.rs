//! Profile CLI 子命令协议。
//!
//! 本模块只定义 Clap 解析后的数据，不读取配置、不执行文件操作，也不负责输出。
//!
//! 作者：SongZQ

use clap::Subcommand;

/// `profile` 分组下的命令参数。
///
/// 命令值在入口解析后传递给 `ProfileHandler`；该类型不保存上下文，避免参数和执行
/// 状态互相耦合。
#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// 列出根目录下可用 Profile。
    List,
    /// 创建一个命名 Profile。
    Create {
        /// 要创建的 Profile 名称。
        name: String,
    },
    /// 将命名 Profile 写为当前选择。
    Use {
        /// 要激活的 Profile 名称。
        name: String,
    },
}
