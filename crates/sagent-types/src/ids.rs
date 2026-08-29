use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(String);

#[cfg(test)]
mod tests {
    use super::{MessageId, SessionId};

    #[test]
    fn session_id_serializes_as_a_plain_json_string() {
        let id = SessionId("20260829_123000_abcdefgh".to_owned());

        assert_eq!(
            serde_json::to_string(&id).expect("应能序列化"),
            "\"20260829_123000_abcdefgh\""
        );
    }

    #[test]
    fn message_id_deserializes_from_a_plain_json_string() {
        let id: MessageId = serde_json::from_str("\"message-1\"").expect("应能反序列化");

        assert_eq!(id.0, "message-1");
    }
}
