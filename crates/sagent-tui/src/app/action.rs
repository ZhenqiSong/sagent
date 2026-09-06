//! Reducer 接收的统一 UI 动作。

/// 一个已被输入层或未来 RPC 层规范化的状态变更请求。
///
/// 该枚举不保存 Crossterm event、RPC reader 或数据库句柄，避免外部 I/O 直接绕过 reducer
/// 修改 ViewModel。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)] // 步骤 1 的 Crossterm 输入循环会生成 AppAction；步骤 0 只验证 reducer 边界。
pub enum AppAction {
    /// 用户明确请求退出 TUI；重复请求必须保持幂等。
    QuitRequested,
}
