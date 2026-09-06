//! Turn 的消息记录与工具调用关联。

use crate::prompt::{PromptMessage, PromptRole, PromptToolCall};
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
    pending_tool_calls: HashSet<String>,
    completed_tool_calls: HashSet<String>,
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
        tool_calls: impl IntoIterator<Item = PromptToolCall>,
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
            .any(|call| self.completed_tool_calls.contains(&call.call_id))
        {
            return Err(TranscriptError::DuplicateToolResult);
        }
        // 在消息进入 transcript 的同一步登记 pending 集合。后续 tool result 只有
        // 命中这里的 Provider call_id 才能落入上下文，避免迟到结果串到新一轮。
        self.pending_tool_calls
            .extend(tool_calls.iter().map(|call| call.call_id.clone()));
        let mut message = PromptMessage::new(PromptRole::Assistant, content);
        message.tool_calls = tool_calls;
        self.messages.push(message);
        Ok(())
    }

    pub fn append_tool_result(
        &mut self,
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Result<(), TranscriptError> {
        let content = non_empty(content)?;
        let tool_call_id = tool_call_id.into();
        if self.completed_tool_calls.contains(&tool_call_id) {
            return Err(TranscriptError::DuplicateToolResult);
        }
        // remove 同时完成“存在性检查”和状态迁移；成功后即使相同结果重放，也会
        // 落入 completed 集合而被稳定地识别为重复。
        if !self.pending_tool_calls.remove(&tool_call_id) {
            return Err(TranscriptError::UnknownToolCall);
        }
        self.completed_tool_calls.insert(tool_call_id.clone());
        self.messages
            .push(PromptMessage::tool(content, tool_call_id));
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
    use crate::PromptToolCall;

    fn call(id: &str) -> PromptToolCall {
        PromptToolCall {
            call_id: id.into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "notes.txt"}),
        }
    }

    #[test]
    fn supports_parallel_tool_results() {
        let mut transcript = Transcript::new();
        transcript.append_user("查询").unwrap();
        transcript
            .append_assistant("开始查询", [call("call-1"), call("call-2")])
            .unwrap();
        transcript.append_tool_result("call-1", "结果一").unwrap();
        transcript.append_tool_result("call-2", "结果二").unwrap();
        assert!(!transcript.has_pending_tool_calls());
        transcript.append_assistant("汇总完成", []).unwrap();
        assert_eq!(transcript.messages().len(), 5);
    }

    #[test]
    fn rejects_unknown_and_duplicate_tool_results() {
        let mut transcript = Transcript::new();
        transcript.append_user("执行").unwrap();
        transcript
            .append_assistant("调用工具", [call("call-1")])
            .unwrap();
        transcript.append_tool_result("call-1", "完成").unwrap();
        assert_eq!(
            transcript.append_tool_result("call-1", "重复"),
            Err(TranscriptError::DuplicateToolResult)
        );
        assert_eq!(
            transcript.append_tool_result("call-missing", "未知"),
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
        transcript
            .append_assistant("工具", [call("call-1")])
            .unwrap();
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
