//! Reducer 接收的统一 UI 动作。

use sagent_protocol::{
    ApprovalDecisionDto, PromptSubmitResult, SessionListResult, SessionResumeResult,
};

/// picker 内的相对移动方向，避免输入层泄漏 Crossterm 键值。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionDirection {
    /// 向上一项移动。
    Up,
    /// 向下一项移动。
    Down,
}

/// 一个已被输入层或未来 RPC 层规范化的状态变更请求。
///
/// 该枚举不保存 Crossterm event、RPC reader 或数据库句柄，避免外部 I/O 直接绕过 reducer
/// 修改 ViewModel。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppAction {
    /// 用户明确请求退出 TUI；重复请求必须保持幂等。
    QuitRequested,
    /// RPC 子进程已创建，UI 应显示启动中状态。
    RpcStarting,
    /// 子进程已启动，正在等待服务端的首个 `gateway.ready` 事件。
    RpcWaitingForReady,
    /// 已收到 `gateway.ready`，下一步应开始 hello 协商。
    GatewayReady,
    /// `client.hello` 已成功协商；此时才允许交互式方法。
    HelloSucceeded,
    /// stdout、子进程或协议校验失败；连接上的未完成请求均已取消。
    RpcDisconnected {
        /// 面向状态栏的脱敏错误摘要。
        message: String,
    },
    /// 握手完成后打开会话 picker，并请求加载当前 Profile 的摘要列表。
    OpenSessionPicker,
    /// picker 内移动高亮项；reducer 会在列表边界夹紧索引。
    MoveSessionSelection(SelectionDirection),
    /// 用户确认当前高亮会话；controller 会读取当前 state 并调用 `session.resume`。
    ConfirmSessionSelection,
    /// 用户请求新建空会话；controller 必须 create 后再 resume。
    CreateSessionRequested,
    /// 关闭当前 overlay，不修改已经恢复的 transcript。
    DismissOverlay,
    /// `session.list` 成功返回的服务端摘要页。
    SessionsLoaded(SessionListResult),
    /// `session.resume` 成功返回的完整可见消息快照。
    /// 装箱避免大型 transcript 快照扩大每一个轻量键盘 action 的内存占用。
    SessionResumed(Box<SessionResumeResult>),
    /// 会话请求失败；保留 picker 与已有 transcript，方便用户重试。
    SessionOperationFailed {
        /// 面向 UI 的稳定、脱敏错误摘要。
        message: String,
    },
    /// 插入一段用户文本；粘贴也必须作为一个 action 进入，避免逐字符副作用。
    ComposerInsert(String),
    /// 删除光标左侧的一个 Unicode 字符。
    ComposerBackspace,
    /// 删除光标右侧的一个 Unicode 字符。
    ComposerDelete,
    /// 光标向左移动一个 Unicode 字符。
    ComposerMoveLeft,
    /// 光标向右移动一个 Unicode 字符。
    ComposerMoveRight,
    /// 用户请求提交 composer；controller 负责校验和 RPC 调用。
    SubmitPromptRequested,
    /// `prompt.submit` 返回的 Turn 关联事实。
    PromptSubmitted(PromptSubmitResult),
    /// 匹配当前 session/turn 的瞬态模型文本。
    StreamDeltaReceived {
        /// 事件归属会话。
        session_id: String,
        /// 事件归属 Turn。
        turn_id: String,
        /// 本次新增文本。
        text: String,
    },
    /// 当前 Turn 已到终态，应由 controller 触发一次 resume。
    ActiveTurnFinished {
        /// 终态事件归属会话。
        session_id: String,
        /// 终态事件归属 Turn。
        turn_id: String,
    },
    /// 活跃 Turn 中用户请求中断；controller 负责发送 RPC。
    InterruptRequested,
    /// interrupt 请求已经被 RPC 接受，但最终状态仍由 event 宣布。
    InterruptAccepted,
    /// Runtime 请求用户审批已脱敏工具调用。
    ApprovalRequested {
        /// Approval、会话和 Turn 的服务端关联字段。
        approval_id: String,
        session_id: String,
        turn_id: String,
        tool_name: String,
        summary: String,
        expires_at: String,
    },
    /// 用户在审批弹层中选择范围。
    ApprovalDecisionRequested(ApprovalDecisionDto),
    /// approval.respond 已被 Runtime mailbox 接受。
    ApprovalResponseAccepted,
    /// 工具活动状态栏更新。
    ToolActivityReceived {
        call_id: String,
        tool_name: String,
        running: Option<bool>,
    },
}
