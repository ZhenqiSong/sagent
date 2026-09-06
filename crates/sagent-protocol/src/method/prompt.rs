//! `prompt.*` 方法的协议 DTO。

use sagent_types::{SessionId, TurnId};
use serde::{Deserialize, Serialize};

/// `prompt.submit` 参数。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptSubmitParams {
    pub session_id: SessionId,
    /// Runtime 接线时拒绝空白输入。
    pub text: String,
}

/// `prompt.submit` 的稳定状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSubmitStatus {
    Streaming,
}

/// 立即响应；最终内容由 event 发送。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSubmitResult {
    pub status: PromptSubmitStatus,
    pub turn_id: TurnId,
}

#[cfg(test)]
mod tests {
    use super::PromptSubmitParams;
    use serde_json::json;

    #[test]
    fn submit_rejects_unknown_fields_before_runtime_receives_input() {
        assert!(
            serde_json::from_value::<PromptSubmitParams>(json!({
                "session_id": "session-1", "text": "你好", "home": "C:/must-not-be-accepted"
            }))
            .is_err()
        );
    }
}
