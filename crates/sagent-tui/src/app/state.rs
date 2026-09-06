//! TUI 的最小可绘制状态。

use sagent_protocol::SessionSummaryDto;

/// TUI 与 RPC transport 的连接可见状态。
///
/// 初始阶段尚未创建子进程，使用 `NotStarted` 明确区分“未尝试连接”与未来的失败/断线。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ConnectionStatus {
    /// 尚未启动或连接 `sagent-rpc`。
    #[default]
    NotStarted,
    /// 已创建 RPC 子进程，尚未收到它的第一条协议帧。
    Starting,
    /// 正在等待服务端先发出的 `gateway.ready` 事件。
    WaitingForReady,
    /// 已收到 ready，正在请求 `client.hello` 协商连接能力。
    Handshaking,
    /// 握手完成；后续步骤可安全调用交互式 RPC 方法。
    Connected,
    /// 子进程或协议流已不可用；文本是可安全展示的诊断摘要。
    Disconnected {
        /// 不含密钥、完整 stderr 或回溯信息的稳定错误摘要。
        message: String,
        /// 自动重连前的退避秒数；零表示可立即手动重试。
        retry_after_secs: u32,
    },
}

/// 可绘制的消息条目；其内容完全来自 `session.resume` 的服务端快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscriptEntry {
    /// SQLite 消息 id 的协议表示，仅用于稳定的 UI 条目身份，不参与本地查询。
    pub message_id: i64,
    /// 服务端返回的消息角色，例如 `user`、`assistant` 或 `tool`。
    pub role: String,
    /// 已通过服务端可见性规则筛选的消息正文。
    pub content: String,
    /// 消息创建时间；旧数据可能缺失。
    pub timestamp: Option<String>,
    /// 服务端定义的展示类别；复杂渲染留给后续步骤。
    pub display_kind: Option<String>,
}

/// 当前已恢复会话的只读 ViewModel。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionView {
    /// 来自 `session.resume` 的会话摘要。
    pub summary: SessionSummaryDto,
    /// 按服务端返回顺序排列的可见消息。
    pub transcript: Vec<TranscriptEntry>,
}

/// 覆盖在主 transcript 上方的交互界面。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Overlay {
    /// 当前没有弹层，主区域显示 transcript 或空提示。
    #[default]
    None,
    /// 会话选择器；选择位置只在内存中保存，不作为会话持久化事实。
    SessionPicker {
        /// 当前高亮项；空列表时恒为零。
        selected_index: usize,
        /// RPC 请求期间阻止重复 Enter/n，避免创建重复会话。
        loading: bool,
    },
    /// 等待用户处理的、由 Runtime 已脱敏的工具审批请求。
    Approval(ApprovalView),
}

/// 审批弹层的纯展示状态；TUI 只保存服务端事件给出的脱敏字段。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalView {
    /// Runtime 生成的审批标识。
    pub approval_id: String,
    /// 审批归属的会话与 Turn，用于拒绝串线事件。
    pub session_id: String,
    /// 审批归属的 Turn。
    pub turn_id: String,
    /// 请求执行的工具名称。
    pub tool_name: String,
    /// Runtime 脱敏后的用户可读摘要。
    pub summary: String,
    /// 审批自动失效时间。
    pub expires_at: String,
    /// 防止在 response 尚未返回时重复提交决定。
    pub submitting: bool,
    /// 最近一次提交失败的安全摘要。
    pub error: Option<String>,
}

/// 当前 Turn 的最近工具活动，仅用于状态栏，不表示工具执行权属于 TUI。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolActivityView {
    /// Runtime 提供的工具调用标识。
    pub call_id: String,
    /// 工具名称。
    pub tool_name: String,
    /// 可显示的生命周期阶段。
    pub status: ToolActivityStatus,
}

/// 工具活动的可见状态。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolActivityStatus {
    /// Provider 已请求工具，但尚未执行。
    Requested,
    /// Runtime 已通过检查并开始执行。
    Running,
    /// 工具已结束；`ok` 只反映 Runtime event，不由 TUI 推测。
    Completed { ok: bool },
}

/// 多行输入框的纯状态；光标按 Unicode 标量值而非 UTF-8 字节计数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposerState {
    /// 用户尚未提交的原始输入；请求失败时必须保留。
    pub text: String,
    /// 位于 `text.chars()` 边界的光标位置。
    pub cursor_char_index: usize,
}

/// 当前正在生成的 Turn 的瞬态视图；文本不会写入本地 transcript。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveTurnView {
    /// 服务端事件必须匹配的会话标识。
    pub session_id: String,
    /// `prompt.submit` 响应返回的唯一 Turn 标识。
    pub turn_id: String,
    /// 尚未由 `session.resume` 确认的瞬态文本片段。
    pub stream_text: String,
    /// 已发送 interrupt，等待终态 event 时禁止重复请求。
    pub interrupt_pending: bool,
    /// 当前或最近的工具活动。
    pub tool_activity: Option<ToolActivityView>,
}

/// 已确认的持久化事件检查点；sequence 与消息 id 不能混用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionCheckpoint {
    /// 检查点所属会话。
    pub session_id: String,
    /// 最后一条已应用 daemon event 的 sequence。
    pub last_sequence: u64,
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
    /// 当前 Profile 下由 `session.list` 返回的会话摘要。
    pub sessions: Vec<SessionSummaryDto>,
    /// 当前已由 `session.resume` 恢复的会话快照。
    pub active_session: Option<SessionView>,
    /// 当前覆盖层，所有选择器/审批 UI 共用这个单入口。
    pub overlay: Overlay,
    /// 可展示的短错误；不保存原始 stderr、数据库内容或 provider 细节。
    pub status_message: Option<String>,
    /// 多行 composer 的临时编辑状态。
    pub composer: ComposerState,
    /// 活跃 Turn 存在时禁止本会话再次 submit。
    pub active_turn: Option<ActiveTurnView>,
    /// 当前会话持久化事件的恢复位置；瞬态 delta 永远不会写入此字段。
    pub checkpoint: Option<SessionCheckpoint>,
}
