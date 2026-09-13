//! SessionActor 的会话级上下文与策略类型。
//!
//! 这些类型只保存 Actor 运行所需的稳定上下文，不实现命令处理或持久化流程，避免
//! `SessionActor` 同时承担依赖装配、生命周期管理和业务编排等职责。

use sagent_store::{SessionQueryStorage, SessionStorage};
use sagent_types::SessionId;
use tokio::sync::{broadcast, mpsc};

use crate::{approval::ApprovalManager, event::RuntimeEvent, input::ActorInput};

/// Session 级持久化上下文；所有持久化操作使用领域端口和可注入时钟。
///
/// 时钟与端口放在这里是为了让持久化事实拥有统一的时间来源，测试可以替换时钟，
/// 而 Actor 的业务策略和 Turn 状态不会因此耦合到存储实现。
pub(crate) struct ActorSessionContext {
    pub(crate) session_id: SessionId,
    pub(crate) session_storage: Box<dyn SessionStorage>,
    pub(crate) query_storage: Box<dyn SessionQueryStorage>,
    pub(crate) clock: fn() -> String,
}

/// Actor 的 mailbox 与事件出口；worker 结果必须经 mailbox 返回 Actor。
pub(crate) struct ActorChannels {
    pub(crate) command_rx: mpsc::Receiver<ActorInput>,
    pub(crate) command_tx: mpsc::Sender<ActorInput>,
    pub(crate) event_tx: broadcast::Sender<RuntimeEvent>,
}

/// Actor 启动代际与恢复计划；二者共同决定新 Turn 能否开始。
pub(crate) struct ActorLifecycle {
    pub(crate) generation: i64,
    pub(crate) startup_recovery: Result<Option<crate::recovery::RecoveryPlan>, String>,
}

/// Actor 的审批、交互和工具回环策略；只描述运行策略，不拥有时间来源。
///
/// 将这些字段集中在一起可以让策略配置和 Session 状态分离，避免 Actor 的主结构
/// 同时承担运行时资源、生命周期状态和策略参数等不同职责。
pub(crate) struct ActorPolicy {
    pub(crate) approvals: ApprovalManager,
    pub(crate) interactive_approval: bool,
    pub(crate) max_tool_rounds: u32,
}
