//! 不依赖终端或 RPC I/O 的 TUI 纯状态层。
//!
//! 所有外部输入最终都要转换为 action，再经 reducer 修改 state；这使 UI 在没有 raw
//! terminal、子进程或数据库的测试环境中也能验证状态转移契约。

mod action;
mod controller;
mod reducer;
mod state;

pub use action::{AppAction, SelectionDirection};
pub use controller::{handle_action, handle_rpc_event, load_sessions, restore_active_session};
pub use reducer::reduce;
pub use state::{
    ActiveTurnView, AppState, ApprovalView, ConnectionStatus, Overlay, SessionView,
    ToolActivityStatus, ToolActivityView, TranscriptEntry,
};
