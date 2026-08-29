use serde::{Deserialize, Serialize};

use crate::SessionId;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: SessionId,
    pub title: Option<String>,
    pub updated_at: Option<String>,
    pub message_count: i64,
}
