use serde::{Deserialize, Serialize};

use crate::{MessageId, SessionId};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: MessageId,
    pub session_id: SessionId,
    pub role: String,
    pub content: String,
    pub created_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchHit {
    pub session_id: SessionId,
    pub message_id: Option<MessageId>,
    pub snippet: String,
    pub rank: Option<f64>,
}
