//! Provider tool-call 增量的聚合与校验。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// 一个 Provider 回合结束后可执行的完整工具调用。
///
/// `call_id` 保留 Provider 原始字符串（例如 `call_abc`），直到 Store/协议边界再
/// 做映射；不能假设上游 ID 一定是本地 UUID。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Eq, Error, PartialEq)]
pub enum ToolCallError {
    #[error("tool call id 不能为空")]
    EmptyId,
    #[error("tool call name 不能为空")]
    MissingName,
    #[error("同一个 tool call 的 name 不一致")]
    ConflictingName,
    #[error("tool call arguments 不是有效 JSON: {0}")]
    InvalidArguments(String),
    #[error("tool call 增量过大")]
    ArgumentsTooLarge,
}

const MAX_ARGUMENT_BYTES: usize = 256 * 1024;

#[derive(Debug, Default)]
struct PartialToolCall {
    name: Option<String>,
    arguments: String,
}

/// 按 Provider 发送顺序聚合多个 `ToolCallDelta`。
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    calls: Vec<String>,
    index: HashMap<String, usize>,
    partial: HashMap<String, PartialToolCall>,
}

impl ToolCallAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    pub fn push(
        &mut self,
        call_id: impl Into<String>,
        name: Option<String>,
        arguments_delta: impl Into<String>,
    ) -> Result<(), ToolCallError> {
        let call_id = call_id.into();
        if call_id.trim().is_empty() {
            return Err(ToolCallError::EmptyId);
        }
        let arguments_delta = arguments_delta.into();
        let entry = if let Some(index) = self.index.get(&call_id).copied() {
            let _ = index;
            self.partial
                .get_mut(&call_id)
                .expect("索引和 partial 必须一致")
        } else {
            let index = self.calls.len();
            self.calls.push(call_id.clone());
            self.index.insert(call_id.clone(), index);
            self.partial.entry(call_id.clone()).or_default()
        };
        if let Some(name) = name {
            if name.trim().is_empty() {
                return Err(ToolCallError::MissingName);
            }
            if let Some(existing) = &entry.name {
                if existing != &name {
                    return Err(ToolCallError::ConflictingName);
                }
            } else {
                entry.name = Some(name);
            }
        }
        if entry.arguments.len().saturating_add(arguments_delta.len()) > MAX_ARGUMENT_BYTES {
            return Err(ToolCallError::ArgumentsTooLarge);
        }
        entry.arguments.push_str(&arguments_delta);
        Ok(())
    }

    pub fn finish(self) -> Result<Vec<ToolCall>, ToolCallError> {
        self.calls
            .into_iter()
            .map(|call_id| {
                let partial = self.partial.get(&call_id).expect("索引和 partial 必须一致");
                let name = partial.name.clone().ok_or(ToolCallError::MissingName)?;
                let arguments = if partial.arguments.trim().is_empty() {
                    Value::Object(Default::default())
                } else {
                    serde_json::from_str(&partial.arguments)
                        .map_err(|error| ToolCallError::InvalidArguments(error.to_string()))?
                };
                if !arguments.is_object() {
                    return Err(ToolCallError::InvalidArguments(
                        "工具参数必须是 JSON object".into(),
                    ));
                }
                Ok(ToolCall {
                    call_id,
                    name,
                    arguments,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolCallAccumulator, ToolCallError};
    use serde_json::json;

    #[test]
    fn aggregates_arguments_in_provider_order() {
        let mut accumulator = ToolCallAccumulator::new();
        accumulator
            .push("call-a", Some("terminal".into()), "{\"command\":")
            .unwrap();
        accumulator.push("call-a", None, "\"pwd\"}").unwrap();
        accumulator
            .push("call-b", Some("read_file".into()), "{\"path\":\"a\"}")
            .unwrap();
        let calls = accumulator.finish().unwrap();
        assert_eq!(calls[0].call_id, "call-a");
        assert_eq!(calls[0].arguments, json!({"command": "pwd"}));
        assert_eq!(calls[1].name, "read_file");
    }

    #[test]
    fn rejects_missing_name_invalid_json_and_oversized_arguments() {
        let mut missing_name = ToolCallAccumulator::new();
        missing_name.push("call-a", None, "{}").unwrap();
        assert_eq!(missing_name.finish(), Err(ToolCallError::MissingName));

        let mut invalid = ToolCallAccumulator::new();
        invalid
            .push("call-a", Some("terminal".into()), "not-json")
            .unwrap();
        assert!(matches!(
            invalid.finish(),
            Err(ToolCallError::InvalidArguments(_))
        ));

        let mut oversized = ToolCallAccumulator::new();
        assert_eq!(
            oversized.push(
                "call-a",
                Some("terminal".into()),
                "x".repeat(256 * 1024 + 1)
            ),
            Err(ToolCallError::ArgumentsTooLarge)
        );
    }

    #[test]
    fn rejects_empty_id_and_conflicting_names() {
        let mut accumulator = ToolCallAccumulator::new();
        assert_eq!(
            accumulator.push(" ", Some("terminal".into()), "{}"),
            Err(ToolCallError::EmptyId)
        );
        accumulator
            .push("call-a", Some("terminal".into()), "{}")
            .unwrap();
        assert_eq!(
            accumulator.push("call-a", Some("read_file".into()), ""),
            Err(ToolCallError::ConflictingName)
        );
    }
}
