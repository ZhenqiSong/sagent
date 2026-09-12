//! 工具级审批策略。
//!
//! 该模块只做纯分类，不保存 pending 状态，也不等待用户。状态和生命周期由
//! `sagent-runtime::ApprovalManager` 管理。

use serde_json::Value;

use crate::{CommandRisk, classify_command};

/// 工具进入执行器前的策略结论。
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ApprovalPolicyDecision {
    /// 可以直接执行。
    Allow,
    /// 必须先创建审批请求。
    RequireApproval { policy_key: String, summary: String },
    /// 明确禁止，不能通过用户审批绕过。
    Deny { reason: String },
}

/// 根据工具名和 JSON 参数分类。
///
/// `read_file` 的路径边界由 `ReadFileService` 继续校验；这里不因为文件读取本身
/// 创建审批卡片。未知工具 fail-closed，避免漏掉未注册工具的权限判断。
pub fn classify_tool(tool_name: &str, arguments: &Value) -> ApprovalPolicyDecision {
    match tool_name {
        "read_file" => ApprovalPolicyDecision::Allow,
        "session_search" => ApprovalPolicyDecision::Allow,
        // 文件写入没有像 terminal 那样可由命令文本细分的风险等级；所有写入均须由
        // Runtime 创建审批记录，避免模型通过 overwrite 静默改变已有工作区内容。
        "write_file" => ApprovalPolicyDecision::RequireApproval {
            policy_key: "write_file:workspace".into(),
            summary: "该 write_file 操作需要用户审批".into(),
        },
        "terminal" => {
            let Some(command) = arguments.get("command").and_then(Value::as_str) else {
                return ApprovalPolicyDecision::Deny {
                    reason: "terminal.command 必须是字符串".into(),
                };
            };
            match classify_command(command) {
                CommandRisk::Safe => ApprovalPolicyDecision::Allow,
                CommandRisk::RequireApproval {
                    policy_key,
                    summary,
                } => ApprovalPolicyDecision::RequireApproval {
                    policy_key,
                    summary,
                },
                CommandRisk::Deny { reason } => ApprovalPolicyDecision::Deny { reason },
            }
        }
        _ => ApprovalPolicyDecision::Deny {
            reason: format!("未知工具：{tool_name}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{ApprovalPolicyDecision, classify_tool};
    use serde_json::json;

    #[test]
    fn read_file_is_allowed_before_path_validation() {
        assert_eq!(
            classify_tool("read_file", &json!({"path": "README.md"})),
            ApprovalPolicyDecision::Allow
        );
    }

    #[test]
    fn terminal_danger_is_promoted_to_approval() {
        assert!(matches!(
            classify_tool("terminal", &json!({"command": "rm -rf build"})),
            ApprovalPolicyDecision::RequireApproval { .. }
        ));
    }

    #[test]
    fn write_file_always_requires_explicit_approval() {
        assert!(matches!(
            classify_tool("write_file", &json!({"path": "note.txt", "content": "secret"})),
            ApprovalPolicyDecision::RequireApproval { ref policy_key, .. }
                if policy_key == "write_file:workspace"
        ));
    }

    #[test]
    fn malformed_or_unknown_tools_fail_closed() {
        assert!(matches!(
            classify_tool("terminal", &json!({})),
            ApprovalPolicyDecision::Deny { .. }
        ));
        assert!(matches!(
            classify_tool("unknown", &json!({})),
            ApprovalPolicyDecision::Deny { .. }
        ));
    }
}
