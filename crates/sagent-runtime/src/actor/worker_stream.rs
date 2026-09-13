//! SessionActor 的 worker 流式输出与用量事件处理。
//!
//! 这类事件只影响实时展示，不改变 Turn 持久化状态；处理器仍会检查 active Turn 和
//! 取消令牌，确保旧 worker 或取消后的迟到片段不会再次对外可见。

use sagent_provider::TokenUsage;
use sagent_types::TurnId;

use super::SessionActor;
use crate::event::{RuntimeEvent, RuntimeEventKind};

impl SessionActor {
    /// 发布当前 Turn 的一个流式文本片段；不存在当前 Turn 时静默丢弃迟到事件。
    pub(super) fn publish_text_delta(&self, turn_id: TurnId, text: String) {
        if !self.is_active_turn(turn_id) || self.is_cancelled(turn_id) {
            return;
        }
        self.publish(RuntimeEvent {
            session_id: self.context.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: self.active.as_ref().map(|turn| turn.request_id),
            kind: RuntimeEventKind::ModelTextDelta { text },
        });
    }

    /// 发布 Provider 的 token 用量；用量是瞬态诊断，不混入 assistant 正文持久化。
    pub(super) fn publish_usage(&self, turn_id: TurnId, usage: TokenUsage) {
        if !self.is_active_turn(turn_id) || self.is_cancelled(turn_id) {
            return;
        }
        self.publish(RuntimeEvent {
            session_id: self.context.session_id.clone(),
            turn_id: Some(turn_id),
            request_id: self.active.as_ref().map(|turn| turn.request_id),
            kind: RuntimeEventKind::ModelUsage { usage },
        });
    }
}
