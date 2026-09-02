//! Turn 持久化所需的跨 crate 类型。

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

/// Store 需要保存的稳定回合结果状态。
///
/// Agent 运行时的 `TurnState` 还包含 Prompting、工具执行和审批等待等
/// 瞬态状态；这些状态由 Runtime 映射为 `Running` 后再写入数据库。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedTurnStatus {
    Running,
    Completed,
    Interrupted,
    Failed,
}

/// daemon event 的单调递增序号。
#[derive(Debug, Clone, Copy, Default, Eq, Hash, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct EventSequence(i64);

impl EventSequence {
    /// 创建非负事件序号。
    pub fn new(value: i64) -> Result<Self, TurnTypeError> {
        if value < 0 {
            return Err(TurnTypeError::NegativeEventSequence(value));
        }
        Ok(Self(value))
    }

    /// 返回用于 SQL 分页和 RPC checkpoint 的整数值。
    pub fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for EventSequence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// 构造持久化 Turn 类型时的错误。
#[derive(Debug, Clone, Eq, Error, PartialEq)]
pub enum TurnTypeError {
    #[error("事件序号不能为负数：{0}")]
    NegativeEventSequence(i64),
}

#[cfg(test)]
mod tests {
    use super::{EventSequence, PersistedTurnStatus, TurnTypeError};

    #[test]
    fn persisted_status_uses_stable_snake_case_json() {
        assert_eq!(
            serde_json::to_string(&PersistedTurnStatus::Interrupted).unwrap(),
            "\"interrupted\""
        );
        let decoded: PersistedTurnStatus = serde_json::from_str("\"completed\"").unwrap();
        assert_eq!(decoded, PersistedTurnStatus::Completed);
    }

    #[test]
    fn event_sequence_accepts_zero_and_positive_values() {
        assert_eq!(EventSequence::new(0).unwrap().get(), 0);
        let sequence = EventSequence::new(42).unwrap();
        assert_eq!(sequence.get(), 42);
        assert_eq!(serde_json::to_string(&sequence).unwrap(), "42");
    }

    #[test]
    fn event_sequence_rejects_negative_values() {
        assert_eq!(
            EventSequence::new(-1),
            Err(TurnTypeError::NegativeEventSequence(-1))
        );
        assert!(serde_json::from_str::<EventSequence>("-1").is_err());
    }
}
