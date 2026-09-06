//! TUI 的最小可绘制状态。

/// TUI 与 RPC transport 的连接可见状态。
///
/// 初始阶段尚未创建子进程，使用 `NotStarted` 明确区分“未尝试连接”与未来的失败/断线。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ConnectionStatus {
    /// 尚未启动或连接 `sagent-rpc`。
    #[default]
    NotStarted,
}

/// 当前 TUI 的纯 ViewModel 根状态。
///
/// 后续 transcript、composer、overlay 和 session 状态都会由 action/reducer 扩展；这里
/// 不得放入 Store、Provider、终端句柄或异步 task，保证 draw 可以只读地消费它。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppState {
    /// 主循环是否应退出；只由 reducer 的 QuitRequested 修改。
    pub should_quit: bool,
    /// 面向状态栏的连接状态。
    pub status: ConnectionStatus,
}
