//! SessionActor 的 Provider/Tool worker 事件分发入口。
//!
//! 本模块只按事件种类把 worker 事实路由到主题处理器；流式展示、工具回环和终态收口
//! 分别由同级模块实现。worker 只投递事实，所有状态转换、Store 写入和事件发布仍在
//! Actor mailbox 中串行完成，从而阻止迟到结果与终态收口竞争。

use super::SessionActor;
use crate::input::WorkerEvent;

impl SessionActor {
    /// 将 worker 事实路由到单一职责处理器，不在入口混合状态、持久化与执行逻辑。
    ///
    /// mailbox 已经保证事件顺序；处理器仍需自行检查 Turn 是否当前 active，丢弃迟到
    /// 事件，避免旧 worker 在新 Turn 中推进状态。
    pub(super) async fn handle_worker_event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::TextDelta { turn_id, text } => self.publish_text_delta(turn_id, text),
            WorkerEvent::Usage { turn_id, usage } => self.publish_usage(turn_id, usage),
            WorkerEvent::FinalText { turn_id, text } => {
                self.complete_worker_turn(turn_id, text).await
            }
            WorkerEvent::Failed { turn_id, reason } => self.fail_worker_turn(turn_id, reason).await,
            WorkerEvent::Cancelled { turn_id } => self.cancel_worker_turn(turn_id).await,
            WorkerEvent::ToolCalls {
                turn_id,
                text,
                calls,
            } => self.apply_tool_calls(turn_id, text, calls).await,
            WorkerEvent::ToolResults { turn_id, results } => {
                self.apply_tool_results(turn_id, results).await;
            }
        }
    }
}
