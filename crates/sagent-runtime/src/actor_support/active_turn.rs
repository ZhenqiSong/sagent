//! SessionActor 当前活跃 Turn 的运行时状态。

use sagent_agent::{RequestId, SystemPromptParts, TurnState};
use sagent_types::{ApprovalId, TurnId};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::tool_dispatch::ToolDispatchPlan;

/// 一个 Provider 工具批次在 Actor 内的推进游标。
///
/// 已完成的结果只保存在 Store；这里仅保留尚未执行的计划和当前调用是否已获得
/// 一次性批准，从而避免审批恢复时重跑前面的工具。
#[derive(Debug)]
pub(crate) struct PendingToolBatch {
    pub(crate) plans: Vec<ToolDispatchPlan>,
    pub(crate) next_index: usize,
    pub(crate) current_approved: bool,
}

impl PendingToolBatch {
    pub(crate) fn new(plans: Vec<ToolDispatchPlan>) -> Self {
        Self {
            plans,
            next_index: 0,
            current_approved: false,
        }
    }

    pub(crate) fn current(&self) -> Option<&ToolDispatchPlan> {
        self.plans.get(self.next_index)
    }

    pub(crate) fn approve_current(&mut self) {
        self.current_approved = true;
    }

    pub(crate) fn complete_current(&mut self) {
        self.next_index += 1;
        self.current_approved = false;
    }
}

/// Actor 内存中保存的一个正在处理的 Turn。
pub(crate) struct ActiveTurn {
    pub(crate) turn_id: TurnId,
    pub(crate) request_id: RequestId,
    pub(crate) generation: i64,
    /// 同一 Turn 内已完成的工具调用批次；用于限制 Provider → Tool 回环。
    pub(crate) tool_rounds: u32,
    /// 同一 Turn 必须保持 byte-stable 的系统提示词组成部分。
    pub(crate) system: SystemPromptParts,
    pub(crate) state: TurnState,
    pub(crate) cancellation: CancellationToken,
    /// Actor 持有的是 worker 监控任务；只有 panic 等 JoinError 才回传 WorkerExited。
    pub(crate) worker: Option<JoinHandle<()>>,
    /// 指向实际 worker 的取消句柄。这样取消监控任务时不会遗留实际 worker。
    pub(crate) worker_abort: Option<AbortHandle>,
    /// 正在执行当前工具调用的任务。它与 Provider monitor 分开保存，使 interrupt
    /// 能先传播 CancellationToken，再有界等待 terminal 清理其进程树。
    pub(crate) tool_task: Option<JoinHandle<()>>,
    /// 当前 pending approval 的 waiter；Actor 只保存任务句柄，不在 mailbox 中同步等待。
    pub(crate) approval_waiter: Option<JoinHandle<()>>,
    pub(crate) approval_id: Option<ApprovalId>,
    /// 当前 Provider 工具批次；审批期间保留在此处，批准后从当前位置继续。
    pub(crate) pending_tool_batch: Option<PendingToolBatch>,
    /// 防止 Final/Failed/Cancelled/Interrupt 竞争时重复收口。
    pub(crate) terminal: bool,
}
