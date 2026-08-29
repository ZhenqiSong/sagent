use serde::{Deserialize, Serialize};

use crate::SessionId;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: SessionId,
    pub source: Option<String>,
    pub model: Option<String>,
    pub title: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub end_reason: Option<String>,
    pub last_active: Option<String>,
    pub preview: Option<String>,
    pub message_count: i64,
}

#[cfg(test)]
mod tests {
    use super::SessionSummary;
    use serde_json::{Value, json};

    /// 会话摘要是跨 crate 传递的 DTO，JSON 字段名和可空字段必须稳定。
    #[test]
    fn session_summary_round_trips_all_list_fields() {
        let expected = json!({
            "id": "20260829_123000_abcdefgh",
            "source": "cli",
            "model": "gpt-5.6",
            "title": "实现会话仓储",
            "started_at": "2026-08-29T12:30:00Z",
            "ended_at": null,
            "end_reason": null,
            "last_active": "2026-08-29T12:35:00Z",
            "preview": "下一步实现 SessionRepository",
            "message_count": 42
        });

        let summary: SessionSummary =
            serde_json::from_value(expected.clone()).expect("会话摘要应能反序列化");

        let actual: Value = serde_json::to_value(summary).expect("会话摘要应能序列化");
        assert_eq!(actual, expected);
    }
}
