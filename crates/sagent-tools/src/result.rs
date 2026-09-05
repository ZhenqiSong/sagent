//! 工具执行结果的最小数据模型。

use sagent_types::ToolCallId;
use serde::{Deserialize, Serialize};

/// 结果被截断时追加到文本末尾的稳定标记。
pub const TRUNCATION_MARKER: &str = "\n[输出已截断]";

/// 一次工具调用返回给 SessionActor 的有界结果。
///
/// 该类型只描述结果，不负责执行工具、写数据库或发布 RuntimeEvent。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: ToolCallId,
    pub name: String,
    pub ok: bool,
    pub content: String,
    pub truncated: bool,
    pub exit_code: Option<i32>,
    pub error_kind: Option<String>,
}

impl ToolResult {
    /// 创建成功结果，并把 content 限制在 output_limit 个字符以内。
    pub fn success(
        tool_call_id: ToolCallId,
        name: impl Into<String>,
        content: impl Into<String>,
        output_limit: usize,
        exit_code: Option<i32>,
    ) -> Self {
        let (content, truncated) = limit_content(content.into(), output_limit);
        Self {
            tool_call_id,
            name: name.into(),
            ok: true,
            content,
            truncated,
            exit_code,
            error_kind: None,
        }
    }

    /// 创建失败结果；错误正文同样受 output_limit 限制。
    pub fn failure(
        tool_call_id: ToolCallId,
        name: impl Into<String>,
        error_kind: impl Into<String>,
        content: impl Into<String>,
        output_limit: usize,
        exit_code: Option<i32>,
    ) -> Self {
        let (content, truncated) = limit_content(content.into(), output_limit);
        Self {
            tool_call_id,
            name: name.into(),
            ok: false,
            content,
            truncated,
            exit_code,
            error_kind: Some(error_kind.into()),
        }
    }
}

fn limit_content(content: String, output_limit: usize) -> (String, bool) {
    if content.chars().count() <= output_limit {
        return (content, false);
    }

    // output_limit 来自经过校验的 ToolDefinition；即使调用方直接使用构造器，
    // 这里也保证最终内容永远不超过它，而不是无限保留工具输出。
    let marker_len = TRUNCATION_MARKER.chars().count();
    if output_limit <= marker_len {
        return (TRUNCATION_MARKER.chars().take(output_limit).collect(), true);
    }
    let prefix_len = output_limit - marker_len;
    let bounded = content.chars().take(prefix_len).collect::<String>();
    (format!("{bounded}{TRUNCATION_MARKER}"), true)
}

#[cfg(test)]
mod tests {
    use super::{TRUNCATION_MARKER, ToolResult};
    use sagent_types::ToolCallId;

    #[test]
    fn successful_result_is_bounded_and_marks_truncation() {
        let result = ToolResult::success(
            ToolCallId::new(),
            "read_file",
            "abcdefghijklmnopqrst",
            16,
            None,
        );
        assert!(result.ok);
        assert!(result.truncated);
        assert!(result.content.ends_with(TRUNCATION_MARKER));
        assert_eq!(result.content.chars().count(), 16);
    }

    #[test]
    fn short_failure_keeps_error_metadata() {
        let result = ToolResult::failure(
            ToolCallId::new(),
            "terminal",
            "timeout",
            "命令执行超时",
            64,
            None,
        );
        assert!(!result.ok);
        assert_eq!(result.error_kind.as_deref(), Some("timeout"));
        assert!(!result.truncated);
    }
}
