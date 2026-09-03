//! SessionActor 当前活跃 Turn 的运行时状态。

use sagent_agent::{RequestId, TurnState};
use sagent_types::TurnId;
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;

/// Actor 内存中保存的一个正在处理的 Turn。
pub(crate) struct ActiveTurn {
    pub(crate) turn_id: TurnId,
    pub(crate) request_id: RequestId,
    pub(crate) generation: i64,
    pub(crate) state: TurnState,
    pub(crate) cancellation: CancellationToken,
    /// Actor 持有的是 worker 监控任务；监控任务结束时会回传 WorkerExited。
    pub(crate) worker: Option<JoinHandle<()>>,
    /// 指向实际 worker 的取消句柄。这样取消监控任务时不会遗留实际 worker。
    pub(crate) worker_abort: Option<AbortHandle>,
    /// 防止 Final/Failed/Cancelled/Interrupt 竞争时重复收口。
    pub(crate) terminal: bool,
}
