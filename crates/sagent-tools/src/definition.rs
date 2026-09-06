//! 工具定义和模型可见权限元数据。

use crate::RegistryError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// 工具的最小权限分类。
///
/// `ApprovalRequired` 只是声明工具需要经过 Runtime 的审批策略；它不会在
/// `sagent-tools` 内部等待用户，也不会自行绕过审批。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPermission {
    /// 不修改外部状态的只读工具，例如 read_file。
    ReadOnly,
    /// 可能影响宿主环境，需要 Runtime approval policy 决定是否执行。
    ApprovalRequired,
    /// 明确禁止执行的工具定义。
    Deny,
}

/// 注册到 ToolRegistry 的稳定工具元数据。
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// 传给 Provider 的工具名称。
    pub name: String,
    /// 模型可见的工具说明。
    pub description: String,
    /// JSON Schema 格式的参数定义。
    pub input_schema: Value,
    /// Runtime 使用的权限分类。
    pub permission: ToolPermission,
    /// 单次执行的超时时间（毫秒）。
    pub timeout_ms: u64,
    /// 返回给模型的最大字符数。
    pub output_limit: usize,
}

impl ToolDefinition {
    /// 创建并校验一个工具定义。
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        permission: ToolPermission,
        timeout_ms: u64,
        output_limit: usize,
    ) -> Result<Self, RegistryError> {
        let name = name.into();
        validate_name(&name)?;
        if !input_schema.is_object() {
            return Err(RegistryError::InvalidSchema);
        }
        if timeout_ms == 0 {
            return Err(RegistryError::InvalidTimeout);
        }
        if output_limit == 0 {
            return Err(RegistryError::InvalidOutputLimit);
        }
        Ok(Self {
            name,
            description: description.into(),
            input_schema,
            permission,
            timeout_ms,
            output_limit,
        })
    }

    /// 转换成 OpenAI-compatible function tool schema。
    ///
    /// timeout/output_limit/permission 不暴露给模型，但会参与 registry fingerprint，
    /// 这样安全策略变化时不会错误复用旧 generation。
    pub fn model_schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.input_schema,
            }
        })
    }

    /// 返回用于 generation hash 的完整定义快照。
    pub(crate) fn fingerprint_value(&self) -> Value {
        let mut value = Map::new();
        value.insert(
            "description".into(),
            Value::String(self.description.clone()),
        );
        value.insert("input_schema".into(), self.input_schema.clone());
        value.insert("name".into(), Value::String(self.name.clone()));
        value.insert(
            "output_limit".into(),
            Value::Number((self.output_limit as u64).into()),
        );
        value.insert("permission".into(), json!(self.permission));
        value.insert("timeout_ms".into(), Value::Number(self.timeout_ms.into()));
        Value::Object(value)
    }
}

fn validate_name(name: &str) -> Result<(), RegistryError> {
    if name.is_empty() {
        return Err(RegistryError::EmptyName);
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(RegistryError::InvalidName(name.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ToolDefinition, ToolPermission};
    use crate::RegistryError;
    use serde_json::json;

    fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "read_file",
            "读取工作区内的文本文件",
            json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            ToolPermission::ReadOnly,
            30_000,
            32_768,
        )
        .unwrap()
    }

    #[test]
    fn validates_definition_constraints() {
        assert_eq!(
            ToolDefinition::new("", "", json!({}), ToolPermission::ReadOnly, 1, 1),
            Err(RegistryError::EmptyName)
        );
        assert!(matches!(
            ToolDefinition::new("bad.name", "", json!({}), ToolPermission::ReadOnly, 1, 1),
            Err(RegistryError::InvalidName(_))
        ));
        assert_eq!(
            ToolDefinition::new("x", "", json!("not-object"), ToolPermission::ReadOnly, 1, 1),
            Err(RegistryError::InvalidSchema)
        );
        assert_eq!(
            ToolDefinition::new("x", "", json!({}), ToolPermission::ReadOnly, 0, 1),
            Err(RegistryError::InvalidTimeout)
        );
        assert_eq!(
            ToolDefinition::new("x", "", json!({}), ToolPermission::ReadOnly, 1, 0),
            Err(RegistryError::InvalidOutputLimit)
        );
    }

    #[test]
    fn model_schema_uses_openai_function_shape() {
        let value = definition().model_schema();
        assert_eq!(value["type"], "function");
        assert_eq!(value["function"]["name"], "read_file");
        assert_eq!(value["function"]["parameters"]["type"], "object");
        assert!(value["function"].get("timeout_ms").is_none());
    }
}
