//! 与工具顺序和 JSON object key 顺序无关的 canonical schema 序列化。

use crate::{RegistryError, ToolDefinition};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// 递归生成稳定 JSON 文本：object key 排序，array 顺序保持不变。
pub fn canonical_json(value: &Value) -> Result<String, RegistryError> {
    let mut output = String::new();
    write_canonical(value, &mut output)?;
    Ok(output)
}

/// 生成仅包含模型可见 schema 的稳定 JSON 数组。
pub fn canonical_tool_schema(definitions: &[ToolDefinition]) -> Result<String, RegistryError> {
    let mut values: Vec<&ToolDefinition> = definitions.iter().collect();
    values.sort_by(|left, right| left.name.cmp(&right.name));
    let schemas = Value::Array(
        values
            .into_iter()
            .map(ToolDefinition::model_schema)
            .collect(),
    );
    canonical_json(&schemas)
}

/// 生成包含权限/限制配置的 generation fingerprint。
pub fn tool_schema_hash(definitions: &[ToolDefinition]) -> Result<String, RegistryError> {
    let mut values: Vec<&ToolDefinition> = definitions.iter().collect();
    values.sort_by(|left, right| left.name.cmp(&right.name));
    let fingerprint = Value::Array(
        values
            .into_iter()
            .map(ToolDefinition::fingerprint_value)
            .collect(),
    );
    let canonical = canonical_json(&fingerprint)?;
    let digest = Sha256::digest(canonical.as_bytes());
    Ok(format!("sha256:{digest:x}"))
}

fn write_canonical(value: &Value, output: &mut String) -> Result<(), RegistryError> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => output.push_str(
            &serde_json::to_string(value)
                .map_err(|error| RegistryError::Serialization(error.to_string()))?,
        ),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => write_object(values, output)?,
    }
    Ok(())
}

fn write_object(values: &Map<String, Value>, output: &mut String) -> Result<(), RegistryError> {
    let mut keys: Vec<&String> = values.keys().collect();
    keys.sort();
    output.push('{');
    for (index, key) in keys.into_iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        write_canonical(&Value::String(key.clone()), output)?;
        output.push(':');
        write_canonical(
            values
                .get(key)
                .ok_or_else(|| RegistryError::Serialization("schema key disappeared".into()))?,
            output,
        )?;
    }
    output.push('}');
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{canonical_json, canonical_tool_schema, tool_schema_hash};
    use crate::{ToolDefinition, ToolPermission};
    use serde_json::json;

    fn definition(name: &str, schema: serde_json::Value) -> ToolDefinition {
        ToolDefinition::new(
            name,
            format!("{name} tool"),
            schema,
            ToolPermission::ReadOnly,
            1,
            1,
        )
        .unwrap()
    }

    #[test]
    fn canonical_json_sorts_object_keys_but_preserves_array_order() {
        let first = json!({"z": 1, "a": {"y": true, "b": [2, 1]}});
        let second = json!({"a": {"b": [2, 1], "y": true}, "z": 1});
        assert_eq!(
            canonical_json(&first).unwrap(),
            canonical_json(&second).unwrap()
        );
        assert_ne!(
            canonical_json(&first).unwrap(),
            canonical_json(&json!({"a": {"b": [1, 2], "y": true}, "z": 1})).unwrap()
        );
    }

    #[test]
    fn tool_order_does_not_change_schema_or_hash() {
        let first = vec![
            definition("terminal", json!({"type": "object"})),
            definition("read_file", json!({"type": "object"})),
        ];
        let second = vec![first[1].clone(), first[0].clone()];
        assert_eq!(
            canonical_tool_schema(&first).unwrap(),
            canonical_tool_schema(&second).unwrap()
        );
        assert_eq!(
            tool_schema_hash(&first).unwrap(),
            tool_schema_hash(&second).unwrap()
        );
    }

    #[test]
    fn changing_permission_changes_generation_hash() {
        let read = definition("terminal", json!({"type": "object"}));
        let mut approval = read.clone();
        approval.permission = ToolPermission::ApprovalRequired;
        assert_ne!(
            tool_schema_hash(&[read]).unwrap(),
            tool_schema_hash(&[approval]).unwrap()
        );
    }
}
