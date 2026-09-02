//! Turn 的消息记录与工具调用关联。

use crate::prompt::{PromptMessage, PromptRole};
use sagent_types::ToolCallId;
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Clone, Eq, Error, PartialEq)]
pub enum TranscriptError {
    #[error("消息内容不能为空")]
    EmptyContent,
    #[error("当前消息角色顺序不合法")]
    InvalidRoleOrder,
    #[error("工具调用不存在或已完成")]
    UnknownToolCall,
    #[error("工具结果已经提交过")]
    DuplicateToolResult,
    #[error("仍有未完成的工具调用")]
    PendingToolCalls,
}

/// 一个 Turn 内按 Provider 顺序排列的消息记录。
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct Transcript {
    messages: Vec<PromptMessage>,
    pending_tool_calls: HashSet<ToolCallId>,
    completed_tool_calls: HashSet<ToolCallId>,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append_user(&mut self, content: impl Into<String>) -> Result<(), TranscriptError> {
        let content = non_empty(content)?;
        if self.pending() {
            return Err(TranscriptError::PendingToolCalls);
        }
        if self.last_role() == Some(PromptRole::User) {
            return Err(TranscriptError::InvalidRoleOrder);
        }
        self.messages
            .push(PromptMessage::new(PromptRole::User, content));
        Ok(())
    }

    pub fn append_assistant(
        &mut self,
        content: impl Into<String>,
        tool_calls: impl IntoIterator<Item = ToolCallId>,
    ) -> Result<(), TranscriptError> {
        let content = non_empty(content)?;
        if self.pending() {
            return Err(TranscriptError::PendingToolCalls);
        }
        if self.last_role() == Some(PromptRole::Assistant) {
            return Err(TranscriptError::InvalidRoleOrder);
        }
        let tool_calls: Vec<_> = tool_calls.into_iter().collect();
        if tool_calls
            .iter()
            .any(|id| self.completed_tool_calls.contains(id))
        {
            return Err(TranscriptError::DuplicateToolResult);
        }
        self.pending_tool_calls.extend(tool_calls.iter().copied());
        let mut message = PromptMessage::new(PromptRole::Assistant, content);
        message.tool_calls = tool_calls;
        self.messages.push(message);
        Ok(())
    }

    pub fn append_tool_result(
        &mut self,
        tool_call_id: ToolCallId,
        content: impl Into<String>,
    ) -> Result<(), TranscriptError> {
        let content = non_empty(content)?;
        if self.completed_tool_calls.contains(&tool_call_id) {
            return Err(TranscriptError::DuplicateToolResult);
        }
        if !self.pending_tool_calls.remove(&tool_call_id) {
            return Err(TranscriptError::UnknownToolCall);
        }
        self.completed_tool_calls.insert(tool_call_id);
        self.messages.push(PromptMessage::tool(
            content,
            tool_call_id.as_uuid().to_string(),
        ));
        Ok(())
    }

    pub fn messages(&self) -> &[PromptMessage] {
        &self.messages
    }
    pub fn has_pending_tool_calls(&self) -> bool {
        self.pending()
    }
    fn pending(&self) -> bool {
        !self.pending_tool_calls.is_empty()
    }
    fn last_role(&self) -> Option<PromptRole> {
        self.messages.last().map(|message| message.role)
    }
}

fn non_empty(content: impl Into<String>) -> Result<String, TranscriptError> {
    let content = content.into();
    if content.trim().is_empty() {
        Err(TranscriptError::EmptyContent)
    } else {
        Ok(content)
    }
}

#[cfg(test)]
mod tests {
    use super::{Transcript, TranscriptError};
    use sagent_types::ToolCallId;

    #[test]
    fn supports_parallel_tool_results() {
        let mut transcript = Transcript::new();
        let first = ToolCallId::new();
        let second = ToolCallId::new();
        transcript.append_user("查询").unwrap();
        transcript
            .append_assistant("开始查询", [first, second])
            .unwrap();
        transcript.append_tool_result(first, "结果一").unwrap();
        transcript.append_tool_result(second, "结果二").unwrap();
        assert!(!transcript.has_pending_tool_calls());
        transcript.append_assistant("汇总完成", []).unwrap();
        assert_eq!(transcript.messages().len(), 5);
    }

    #[test]
    fn rejects_unknown_and_duplicate_tool_results() {
        let mut transcript = Transcript::new();
        let id = ToolCallId::new();
        transcript.append_user("执行").unwrap();
        transcript.append_assistant("调用工具", [id]).unwrap();
        transcript.append_tool_result(id, "完成").unwrap();
        assert_eq!(
            transcript.append_tool_result(id, "重复"),
            Err(TranscriptError::DuplicateToolResult)
        );
        assert_eq!(
            transcript.append_tool_result(ToolCallId::new(), "未知"),
            Err(TranscriptError::UnknownToolCall)
        );
    }

    #[test]
    fn rejects_role_order_violations_and_pending_tools() {
        let mut transcript = Transcript::new();
        assert_eq!(
            transcript.append_user(" "),
            Err(TranscriptError::EmptyContent)
        );
        transcript.append_user("问题").unwrap();
        assert_eq!(
            transcript.append_user("再次提问"),
            Err(TranscriptError::InvalidRoleOrder)
        );
        let id = ToolCallId::new();
        transcript.append_assistant("工具", [id]).unwrap();
        assert_eq!(
            transcript.append_user("插入"),
            Err(TranscriptError::PendingToolCalls)
        );
        assert_eq!(
            transcript.append_assistant("插入", []),
            Err(TranscriptError::PendingToolCalls)
        );
    }
}
