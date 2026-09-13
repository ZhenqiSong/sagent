//! SessionActor 的 worker 终态与失败收口。
//!
//! Provider worker 只能投递最终文本、失败或取消事实；本模块把这些事实交给 Turn
//! 终态操作。真正的 Store 写入、取消传播和终态事件顺序由 `turn` 模块统一维护。

use sagent_types::TurnId;

use super::SessionActor;
use crate::input::WorkerFailure;

impl SessionActor {
    /// 将 Provider 的最终文本交给统一 Turn 完成路径，避免 worker 绕过 Actor 写状态。
    pub(super) async fn complete_worker_turn(&mut self, turn_id: TurnId, text: String) {
        let _ = self.complete_active(turn_id, text).await;
    }

    /// 将 worker 的可控失败映射到稳定的 Runtime 失败类别。
    pub(super) async fn fail_worker_turn(&mut self, turn_id: TurnId, reason: String) {
        let _ = self.fail_active(turn_id, "worker", reason).await;
    }

    /// 将 worker 对取消令牌的响应交给 Actor 收口，保持取消先传播再发布终态事件。
    pub(super) async fn cancel_worker_turn(&mut self, turn_id: TurnId) {
        let _ = self
            .interrupt_active_for_turn(turn_id, "worker cancelled")
            .await;
    }

    /// 处理 worker task 的退出通知；正常退出但没有终态事件也必须收口为失败。
    pub(super) async fn handle_worker_exited(
        &mut self,
        turn_id: TurnId,
        result: Result<(), WorkerFailure>,
    ) {
        if !self.is_active_turn(turn_id) || self.is_cancelled(turn_id) {
            return;
        }
        if let Err(error) = result {
            let _ = self.fail_active(turn_id, "worker", error.0).await;
        } else {
            let _ = self
                .fail_active(turn_id, "worker_exit", "worker 未产生最终结果".into())
                .await;
        }
    }
}
