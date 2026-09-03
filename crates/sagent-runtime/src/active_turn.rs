//! SessionActor 当前活跃 Turn 的运行时状态。

use sagent_agent::{RequestId, TurnState};
use sagent_types::TurnId;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Actor 内存中保存的一个正在处理的 Turn。
pub(crate) struct ActiveTurn {
    pub(crate) turn_id: TurnId,
    pub(crate) request_id: RequestId,
    pub(crate) generation: i64,
    pub(crate) state: TurnState,
    pub(crate) cancellation: CancellationToken,
    pub(crate) worker: Option<JoinHandle<()>>,
}
