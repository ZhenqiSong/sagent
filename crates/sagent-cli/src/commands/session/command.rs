//! Session CLI 子命令协议。
//!
//! 本模块只定义 Clap 解析后的命令数据，不读取配置、不打开存储，也不执行会话操作。
//!
//! 作者：SongZQ

use clap::Subcommand;

/// `session` 分组下的命令参数。
///
/// 命令值在入口解析后传递给 `SessionHandler`；该类型不保存运行上下文，避免参数和
/// 会话状态互相耦合。
#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    /// 创建新的空会话。
    Create {
        /// 可选的人类可读标题。
        #[arg(long)]
        title: Option<String>,
        /// 可选的模型标识。
        #[arg(long)]
        model: Option<String>,
    },
    /// 按 ID 查看可见消息。
    Show {
        /// 目标会话 ID。
        session_id: String,
        /// 返回消息条数。
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    /// 在 FTS5 索引中搜索消息。
    Search {
        /// 搜索表达式。
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// 列出会话摘要。
    List {
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        offset: u32,
        /// 同时列出已归档会话；隐藏会话仍不会显示。
        #[arg(long)]
        include_archived: bool,
    },
    /// 更新会话标题。
    Rename { session_id: String, title: String },
    /// 将会话归档。
    Archive { session_id: String },
    /// 取消会话归档。
    Unarchive { session_id: String },
    /// 设置会话结束原因并结束会话。
    Finish {
        session_id: String,
        #[arg(long)]
        reason: String,
    },
    /// 将当前可见消息回退到指定检查点。
    Rewind {
        session_id: String,
        message_id: String,
    },
    /// 从回退检查点恢复消息可见性。
    Restore {
        session_id: String,
        message_id: String,
    },
}
