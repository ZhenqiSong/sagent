//! Terminal 命令的纯风险分类。
//!
//! 这里只判断命令是否需要后续审批，不保存 Session 状态，也不等待用户。

/// 命令风险分类结果。
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CommandRisk {
    /// 可以直接交给进程执行器。
    Safe,
    /// 需要由后续 ApprovalManager 决定是否执行。
    RequireApproval { policy_key: String, summary: String },
    /// 明确拒绝，不进入审批等待。
    Deny { reason: String },
}

/// 对命令做 fail-closed 的最小风险分类。
pub fn classify_command(command: &str) -> CommandRisk {
    let normalized = command.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return CommandRisk::Deny {
            reason: "命令不能为空".into(),
        };
    }

    for (pattern, reason) in [
        ("shutdown", "关机命令被拒绝"),
        ("reboot", "重启命令被拒绝"),
        ("mkfs", "格式化文件系统命令被拒绝"),
        ("diskpart", "磁盘分区命令被拒绝"),
        ("format c:", "格式化系统盘命令被拒绝"),
    ] {
        if normalized.contains(pattern) {
            return CommandRisk::Deny {
                reason: reason.into(),
            };
        }
    }

    for (pattern, policy_key) in [
        ("rm -rf", "terminal:recursive_delete"),
        ("del /s", "terminal:recursive_delete"),
        ("rmdir /s", "terminal:recursive_delete"),
        ("sudo ", "terminal:privilege_escalation"),
        ("chmod 777", "terminal:permission_change"),
        ("git push --force", "terminal:force_push"),
        (".ssh/", "terminal:sensitive_path"),
        (".ssh\\", "terminal:sensitive_path"),
    ] {
        if normalized.contains(pattern) {
            return CommandRisk::RequireApproval {
                policy_key: policy_key.into(),
                summary: "该 terminal 命令需要用户审批".into(),
            };
        }
    }

    if (normalized.contains("curl") || normalized.contains("wget"))
        && (normalized.contains("| sh")
            || normalized.contains("|sh")
            || normalized.contains("| bash")
            || normalized.contains("|bash"))
    {
        return CommandRisk::RequireApproval {
            policy_key: "terminal:piped_download".into(),
            summary: "该 terminal 命令需要用户审批".into(),
        };
    }

    CommandRisk::Safe
}

#[cfg(test)]
mod tests {
    use super::{CommandRisk, classify_command};

    #[test]
    fn safe_command_is_not_blocked() {
        assert_eq!(classify_command("cargo test"), CommandRisk::Safe);
    }

    #[test]
    fn dangerous_command_requires_approval_without_running() {
        assert!(matches!(
            classify_command("rm -rf build"),
            CommandRisk::RequireApproval { .. }
        ));
    }

    #[test]
    fn clearly_destructive_command_is_denied() {
        assert!(matches!(
            classify_command("shutdown /s /t 0"),
            CommandRisk::Deny { .. }
        ));
    }
}
