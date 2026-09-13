//! SessionActor 的 Turn 终态收口与活动状态辅助。
//!
//! 所有终态转换都先取消受监管任务，再写入 Store，只有持久化成功后才发布事件；这条
//! 顺序是 Actor 并发安全和恢复一致性的核心约束。

use std::time::Duration;

use sagent_store::NewMessage;
use sagent_types::TurnId;

use super::SessionActor;
use crate::{
    RuntimeError,
    active_turn::ActiveTurn,
    event::{RuntimeEvent, RuntimeEventKind},
    input::CommandReply,
};

impl SessionActor {
    pub(super) async fn interrupt_active(
        &mut self,
        reason: &str,
    ) -> Result<CommandReply, RuntimeError> {
        let turn_id = self
            .active
            .as_ref()
            .map(|active| active.turn_id)
            .ok_or(RuntimeError::NoActiveTurn)?;
        self.interrupt_active_for_turn(turn_id, reason).await
    }

    /// 将指定的活跃 Turn 收口为 interrupted，并保证终态只在持久化成功后对外可见。
    ///
    /// 此函数先把 `active` 暂时移出 Actor，阻止 worker、审批回调与其它命令在收口期间
    /// 重复处理同一 Turn；随后依次取消执行链、停止受监管任务、原子写入 Store，最后才
    /// 发布 `TurnInterrupted`。若持久化失败，必须恢复 `active`，使调用方得到错误而不把
    /// 内存状态伪装成已结束；成功时不再放回它，Actor 因而回到可接受下一轮提交的空闲状态。
    ///
    /// 不会写入虚构的 assistant 消息。`reason` 会持久化为该 Turn 的中断原因，并用于区分
    /// 用户主动取消与会话关闭等终止来源。
    pub(super) async fn interrupt_active_for_turn(
        &mut self,
        turn_id: TurnId,
        reason: &str,
    ) -> Result<CommandReply, RuntimeError> {
        // 先从 Actor 状态中摘下 active：收口期间迟到的 worker/审批事件将无法再匹配
        // 该 Turn，从而不会与中断路径竞争并重复写入终态。
        let Some(mut active) = self.take_active(turn_id) else {
            return Err(RuntimeError::NoActiveTurn);
        };
        if active.terminal {
            // 理论上已终态的 Turn 不应仍在 active 中；保留原状态以免错误路径改变它。
            self.active = Some(active);
            return Err(RuntimeError::NoActiveTurn);
        }
        // 先传播取消并撤销审批，再等待/终止受监管任务，确保没有后台工作在持久化
        // interrupted 后继续产出结果或启动外部进程。
        active.cancellation.cancel();
        self.policy.approvals.cancel_for_turn(&turn_id);
        Self::stop_worker(&mut active).await;
        let timestamp = (self.context.clock)();
        if let Err(error) = self
            .context
            .store
            .interrupt_turn(&turn_id, reason, &timestamp)
        {
            // 数据库仍是终态事实的唯一来源；写入失败时恢复 active，让调用方显式处理
            // 持久化错误，而不是把内存会话静默变成空闲。
            self.active = Some(active);
            return Err(RuntimeError::Persistence(error.to_string()));
        }
        // 只有 Store 成功提交后才发布终态事件。成功路径不放回 active，释放该 Actor
        // 以接受后续 Turn。
        active.terminal = true;
        self.publish(RuntimeEvent {
            session_id: self.context.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(active.request_id),
            kind: RuntimeEventKind::TurnInterrupted,
        });
        Ok(CommandReply::Interrupted)
    }

    pub(super) async fn complete_active(
        &mut self,
        turn_id: TurnId,
        text: String,
    ) -> Result<(), RuntimeError> {
        let Some(mut active) = self.take_active(turn_id) else {
            return Ok(());
        };
        if active.terminal || active.cancellation.is_cancelled() {
            self.active = Some(active);
            return Ok(());
        }
        let timestamp = (self.context.clock)();
        let message = NewMessage::new(
            self.context.session_id.clone(),
            "assistant",
            text,
            timestamp.clone(),
        );
        let message_id = match self
            .context
            .store
            .complete_turn(&turn_id, &message, &timestamp)
        {
            Ok(message_id) => message_id,
            Err(error) => {
                self.active = Some(active);
                return Err(RuntimeError::Persistence(error.to_string()));
            }
        };
        active.terminal = true;
        self.policy.approvals.cancel_for_turn(&turn_id);
        Self::stop_worker(&mut active).await;
        self.publish(RuntimeEvent {
            session_id: self.context.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(active.request_id),
            kind: RuntimeEventKind::FinalMessagePersisted { message_id },
        });
        self.publish(RuntimeEvent {
            session_id: self.context.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(active.request_id),
            kind: RuntimeEventKind::TurnCompleted,
        });
        Ok(())
    }

    pub(super) async fn fail_active(
        &mut self,
        turn_id: TurnId,
        category: &str,
        reason: String,
    ) -> Result<(), RuntimeError> {
        let Some(mut active) = self.take_active(turn_id) else {
            return Ok(());
        };
        if active.terminal {
            self.active = Some(active);
            return Ok(());
        }
        active.cancellation.cancel();
        self.policy.approvals.cancel_for_turn(&turn_id);
        Self::stop_worker(&mut active).await;
        let timestamp = (self.context.clock)();
        if let Err(error) = self
            .context
            .store
            .fail_turn(&turn_id, category, &reason, &timestamp)
        {
            self.active = Some(active);
            return Err(RuntimeError::Persistence(error.to_string()));
        }
        active.terminal = true;
        self.publish(RuntimeEvent {
            session_id: self.context.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: Some(active.request_id),
            kind: RuntimeEventKind::TurnFailed { reason },
        });
        Ok(())
    }

    fn take_active(&mut self, turn_id: TurnId) -> Option<ActiveTurn> {
        if self.is_active_turn(turn_id) {
            self.active.take()
        } else {
            None
        }
    }

    async fn stop_worker(active: &mut ActiveTurn) {
        // 先让工具观察 cancellation，自行执行 terminal 进程树清理；只有超时才
        // abort task，避免 Tokio abort 直接跳过 TerminalExecutor 的 finally 路径。
        if let Some(mut tool_task) = active.tool_task.take()
            && tokio::time::timeout(Duration::from_secs(3), &mut tool_task)
                .await
                .is_err()
        {
            tool_task.abort();
            let _ = tool_task.await;
        }
        if let Some(abort) = active.worker_abort.take() {
            abort.abort();
        }
        if let Some(worker) = active.worker.take() {
            // 监控任务可能正阻塞在有界 mailbox 的 send 上，先 abort 它再等待。
            worker.abort();
            let _ = worker.await;
        }
        if let Some(waiter) = active.approval_waiter.take() {
            waiter.abort();
            let _ = waiter.await;
        }
        active.approval_id = None;
    }

    pub(super) fn is_cancelled(&self, turn_id: TurnId) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.turn_id == turn_id && active.cancellation.is_cancelled())
    }

    pub(super) fn is_active_turn(&self, turn_id: TurnId) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.turn_id == turn_id)
    }
}
