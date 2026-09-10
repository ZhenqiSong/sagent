//! 将 SessionActor 的实时事件转为 JSON-RPC 通知。

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use sagent_runtime::{RuntimeEvent, RuntimeEventKind, RuntimeEventSubscription};

use crate::stdio::OutboundFrame;

/// 在 submit 响应入队后才允许事件 forwarder 开始发送的控制器。
///
/// 订阅必须早于 `submit`，否则最快的 `PromptAccepted` 会丢失；但 JSON-RPC response
/// 必须先于任何 delta。这个一次性闸门同时满足两个顺序约束。
pub struct EventBridgeGate {
    release: Option<oneshot::Sender<()>>,
    cancellation: Option<CancellationToken>,
}

impl EventBridgeGate {
    /// 在响应已进入同一 outbound FIFO 后放开事件流。
    pub fn release(mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        // 成功放行后 bridge 的生命周期交回 connection scope；Drop 不应再取消它。
        self.cancellation.take();
    }
}

impl Drop for EventBridgeGate {
    fn drop(&mut self) {
        // submit 在 busy、参数错误或 Actor 启动失败时不会放行。显式取消等待 gate 的
        // task，避免它持有 outbound sender 导致 EOF 后 writer 永远无法排空退出。
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
    }
}

/// 启动一个会话事件桥，并返回用于控制其首帧时序的闸门。
///
/// bridge 不拥有 Actor；连接关闭只会取消此订阅和 stdout 转发，不能隐式中断正在
/// 执行的 Turn。终态事件与订阅关闭都会自行结束 task 并释放 outbound sender。
pub fn spawn(
    mut subscription: RuntimeEventSubscription,
    outbound_tx: mpsc::Sender<OutboundFrame>,
    cancellation: CancellationToken,
) -> EventBridgeGate {
    let (release_tx, release_rx) = oneshot::channel();
    let bridge_cancellation = cancellation.child_token();
    let gate_cancellation = bridge_cancellation.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = bridge_cancellation.cancelled() => return,
            _ = release_rx => {}
        }

        loop {
            let event = tokio::select! {
                _ = bridge_cancellation.cancelled() => return,
                event = subscription.recv() => match event {
                    Ok(event) => event,
                    // Actor 停止后没有更多实时事实可发送；客户端可用持久化查询补读。
                    Err(_) => return,
                },
            };
            let terminal = is_terminal(&event.kind);
            let frame = match OutboundFrame::event(&event) {
                Ok(frame) => frame,
                // RuntimeEvent 是本进程的受控数据；序列化失败时终止此 bridge，不能
                // 让一个损坏事件阻塞 stdout 或泄露内部诊断。
                Err(_) => return,
            };
            if outbound_tx.send(frame).await.is_err() {
                return;
            }
            if terminal {
                return;
            }
        }
    });
    EventBridgeGate {
        release: Some(release_tx),
        cancellation: Some(gate_cancellation),
    }
}

/// 为 Runtime 枚举提供稳定的线上事件名；payload 仍保留完整关联字段和数据。
fn event_type(kind: &RuntimeEventKind) -> &'static str {
    match kind {
        RuntimeEventKind::PromptAccepted => "prompt.accepted",
        RuntimeEventKind::UserMessagePersisted { .. } => "message.user.persisted",
        RuntimeEventKind::ModelTextDelta { .. } => "message.delta",
        RuntimeEventKind::ModelUsage { .. } => "model.usage",
        RuntimeEventKind::FinalMessagePersisted { .. } => "message.complete",
        RuntimeEventKind::TurnCompleted => "turn.completed",
        RuntimeEventKind::TurnInterrupted => "turn.interrupted",
        RuntimeEventKind::TurnFailed { .. } => "turn.failed",
        RuntimeEventKind::ApprovalRequested { .. } => "approval.requested",
        RuntimeEventKind::ApprovalResolved { .. } => "approval.resolved",
        RuntimeEventKind::ApprovalTimedOut { .. } => "approval.timed_out",
        RuntimeEventKind::ToolStarted { .. } => "tool.started",
        RuntimeEventKind::ToolCallRequested { .. } => "tool.requested",
        RuntimeEventKind::ToolCompleted { .. } => "tool.completed",
        RuntimeEventKind::ActorStarted => "actor.started",
        RuntimeEventKind::ActorStopped => "actor.stopped",
        RuntimeEventKind::SubscriberLagged { .. } => "subscriber.lagged",
    }
}

/// Turn 终态后当前 bridge 没有继续订阅的价值；下一次成功 submit 会建立新订阅。
fn is_terminal(kind: &RuntimeEventKind) -> bool {
    matches!(
        kind,
        RuntimeEventKind::TurnCompleted
            | RuntimeEventKind::TurnInterrupted
            | RuntimeEventKind::TurnFailed { .. }
    )
}

/// 将一个 RuntimeEvent 变成统一 JSON-RPC event 信封。
pub(crate) fn jsonrpc_event(event: &RuntimeEvent) -> sagent_protocol::JsonRpcEvent<RuntimeEvent> {
    sagent_protocol::JsonRpcEvent::new(event_type(&event.kind), event.clone())
}
