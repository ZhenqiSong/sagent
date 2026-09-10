//! Provider 工具调用进入工具层前的校验与分发规划。

use std::collections::HashSet;
use std::sync::Arc;

use sagent_tools::{ToolPermission, ToolRegistry};
use thiserror::Error;

use crate::ToolCall;

/// 已通过 registry 校验、可以交给工具 worker 的调用计划。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ToolDispatchPlan {
    /// Provider 请求的完整调用。
    pub call: ToolCall,
    /// registry 声明的权限级别。
    pub permission: ToolPermission,
    /// registry 声明的超时限制。
    pub timeout_ms: u64,
    /// registry 声明的输出上限。
    pub output_limit: usize,
}

/// 工具调用在启动 worker 前的稳定错误。
#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum ToolDispatchError {
    #[error("重复的 tool call id：{0}")]
    DuplicateCallId(String),
    #[error("未知工具：{0}")]
    UnknownTool(String),
    #[error("工具被策略禁止：{0}")]
    DeniedTool(String),
    #[error("工具参数必须是 JSON object：{0}")]
    InvalidArguments(String),
}

/// 只读共享的工具分发器。
#[derive(Debug, Clone)]
pub struct ToolDispatcher {
    registry: Arc<ToolRegistry>,
}

impl ToolDispatcher {
    pub fn new(registry: ToolRegistry) -> Self {
        Self {
            registry: Arc::new(registry),
        }
    }

    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// 按 Provider 返回顺序校验并生成计划。
    ///
    /// 任一调用失败都会让整个批次失败，避免只执行半批调用而破坏
    /// assistant(tool_calls) 与 tool message 的严格顺序。
    pub fn plan(&self, calls: Vec<ToolCall>) -> Result<Vec<ToolDispatchPlan>, ToolDispatchError> {
        let mut seen = HashSet::with_capacity(calls.len());
        let mut plans = Vec::with_capacity(calls.len());
        for call in calls {
            if !seen.insert(call.call_id.clone()) {
                return Err(ToolDispatchError::DuplicateCallId(call.call_id));
            }
            if !call.arguments.is_object() {
                return Err(ToolDispatchError::InvalidArguments(call.call_id));
            }
            let definition = self
                .registry
                .get(&call.name)
                .ok_or_else(|| ToolDispatchError::UnknownTool(call.name.clone()))?;
            if definition.permission == ToolPermission::Deny {
                return Err(ToolDispatchError::DeniedTool(call.name));
            }
            plans.push(ToolDispatchPlan {
                call,
                permission: definition.permission,
                timeout_ms: definition.timeout_ms,
                output_limit: definition.output_limit,
            });
        }
        Ok(plans)
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolDispatchError, ToolDispatcher};
    use crate::ToolCall;
    use sagent_tools::{ToolDefinition, ToolPermission, ToolRegistry};
    use serde_json::json;

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        for (name, permission) in [
            ("read_file", ToolPermission::ReadOnly),
            ("blocked", ToolPermission::Deny),
        ] {
            registry
                .register(
                    ToolDefinition::new(
                        name,
                        "测试工具",
                        json!({"type": "object"}),
                        permission,
                        1000,
                        2000,
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        registry
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            call_id: id.into(),
            name: name.into(),
            arguments: json!({"path": "notes.txt"}),
        }
    }

    #[test]
    fn plans_registered_calls_in_provider_order() {
        let plans = ToolDispatcher::new(registry())
            .plan(vec![call("a", "read_file")])
            .unwrap();
        assert_eq!(plans[0].call.call_id, "a");
        assert_eq!(plans[0].permission, ToolPermission::ReadOnly);
        assert_eq!(plans[0].output_limit, 2000);
    }

    #[test]
    fn rejects_duplicate_unknown_denied_and_non_object_calls() {
        let dispatcher = ToolDispatcher::new(registry());
        assert_eq!(
            dispatcher.plan(vec![call("a", "read_file"), call("a", "read_file")]),
            Err(ToolDispatchError::DuplicateCallId("a".into()))
        );
        assert_eq!(
            dispatcher.plan(vec![call("a", "missing")]),
            Err(ToolDispatchError::UnknownTool("missing".into()))
        );
        assert_eq!(
            dispatcher.plan(vec![call("a", "blocked")]),
            Err(ToolDispatchError::DeniedTool("blocked".into()))
        );
        let mut invalid = call("a", "read_file");
        invalid.arguments = json!("not-an-object");
        assert_eq!(
            dispatcher.plan(vec![invalid]),
            Err(ToolDispatchError::InvalidArguments("a".into()))
        );
    }
}
