//! Prompt 快照与消息不变量。

use sagent_types::{SessionId, TurnId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Prompt 中允许出现的消息角色。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptRole {
    System,
    User,
    Assistant,
    Tool,
}

/// 一条发送给 Provider 的消息。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PromptMessage {
    pub role: PromptRole,
    pub content: String,
    /// tool 消息必须带有对应的调用 ID。
    pub tool_call_id: Option<String>,
}

impl PromptMessage {
    pub fn new(role: PromptRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_call_id: None,
        }
    }

    pub fn tool(content: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            role: PromptRole::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// 系统提示词的稳定组成部分。
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SystemPromptParts {
    pub identity: String,
    pub instructions: String,
    pub environment: String,
}

impl SystemPromptParts {
    /// 按固定顺序拼接系统提示词，避免 Hash 因字段顺序变化而漂移。
    pub fn render(&self) -> String {
        format!(
            "{}\n{}\n{}",
            self.identity, self.instructions, self.environment
        )
    }
}

/// 构造 Prompt 快照时的校验错误。
#[derive(Debug, Clone, Eq, Error, PartialEq)]
pub enum PromptError {
    #[error("Prompt 消息列表不能为空")]
    EmptyMessages,
    #[error("system 消息只能位于消息列表开头")]
    SystemMessageNotFirst,
    #[error("相邻消息不能使用相同角色（位置 {index}）")]
    ConsecutiveSameRole { index: usize },
    #[error("tool 消息缺少 tool_call_id（位置 {index}）")]
    ToolMessageMissingCallId { index: usize },
}

/// 某一 Turn 实际发送给模型的不可变 Prompt 快照。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PromptSnapshot {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub messages: Vec<PromptMessage>,
    pub system_prompt_hash: String,
}

impl PromptSnapshot {
    pub fn new(
        session_id: SessionId,
        turn_id: TurnId,
        system: &SystemPromptParts,
        messages: Vec<PromptMessage>,
    ) -> Result<Self, PromptError> {
        validate_messages(&messages)?;
        Ok(Self {
            session_id,
            turn_id,
            messages,
            system_prompt_hash: hash_text(&system.render()),
        })
    }

    /// 判断当前系统提示词是否仍与快照一致。
    pub fn is_system_prompt_compatible(&self, system: &SystemPromptParts) -> bool {
        self.system_prompt_hash == hash_text(&system.render())
    }
}

fn hash_text(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    format!("sha256:{digest:x}")
}

fn validate_messages(messages: &[PromptMessage]) -> Result<(), PromptError> {
    if messages.is_empty() {
        return Err(PromptError::EmptyMessages);
    }
    for (index, message) in messages.iter().enumerate() {
        if message.role == PromptRole::System && index != 0 {
            return Err(PromptError::SystemMessageNotFirst);
        }
        if message.role == PromptRole::Tool && message.tool_call_id.is_none() {
            return Err(PromptError::ToolMessageMissingCallId { index });
        }
        if index > 0 && messages[index - 1].role == message.role {
            return Err(PromptError::ConsecutiveSameRole { index });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PromptError, PromptMessage, PromptRole, PromptSnapshot, SystemPromptParts};
    use sagent_types::{SessionId, TurnId};

    fn system() -> SystemPromptParts {
        SystemPromptParts {
            identity: "你是 Sagent。".into(),
            instructions: "简洁回答。".into(),
            environment: "平台：TUI。".into(),
        }
    }

    fn messages() -> Vec<PromptMessage> {
        vec![
            PromptMessage::new(PromptRole::System, "系统提示词"),
            PromptMessage::new(PromptRole::User, "你好"),
            PromptMessage::new(PromptRole::Assistant, "你好，有什么可以帮你？"),
        ]
    }

    #[test]
    fn same_system_prompt_has_same_hash() {
        let first =
            PromptSnapshot::new(SessionId::new("s"), TurnId::new(), &system(), messages()).unwrap();
        let second =
            PromptSnapshot::new(SessionId::new("s"), TurnId::new(), &system(), messages()).unwrap();
        assert_eq!(first.system_prompt_hash, second.system_prompt_hash);
        assert!(first.is_system_prompt_compatible(&system()));
    }

    #[test]
    fn changing_system_prompt_breaks_compatibility() {
        let snapshot =
            PromptSnapshot::new(SessionId::new("s"), TurnId::new(), &system(), messages()).unwrap();
        let mut changed = system();
        changed.environment = "平台：CLI。".into();
        assert!(!snapshot.is_system_prompt_compatible(&changed));
    }

    #[test]
    fn message_invariants_are_checked() {
        let same_role = vec![
            PromptMessage::new(PromptRole::User, "a"),
            PromptMessage::new(PromptRole::User, "b"),
        ];
        assert!(matches!(
            PromptSnapshot::new(SessionId::new("s"), TurnId::new(), &system(), same_role),
            Err(PromptError::ConsecutiveSameRole { index: 1 })
        ));

        let missing_id = vec![
            PromptMessage::new(PromptRole::System, "s"),
            PromptMessage::new(PromptRole::Tool, "结果"),
        ];
        assert!(matches!(
            PromptSnapshot::new(SessionId::new("s"), TurnId::new(), &system(), missing_id),
            Err(PromptError::ToolMessageMissingCallId { index: 1 })
        ));
    }
}
