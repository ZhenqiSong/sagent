//! 工具定义注册表。

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{RegistryError, ToolDefinition, canonical_tool_schema, tool_schema_hash};

/// 保存当前 generation 可见的工具定义。
///
/// 使用 BTreeMap 保证查询和遍历本身稳定；schema helper 仍会再次按名称排序，
/// 从而不依赖调用方传入顺序。
#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    definitions: BTreeMap<String, ToolDefinition>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册工具；重复名称必须显式由调用方解决，不能静默覆盖。
    pub fn register(&mut self, definition: ToolDefinition) -> Result<(), RegistryError> {
        if self.definitions.contains_key(&definition.name) {
            // 先检查再插入，保证失败的注册不会覆盖已有定义。
            return Err(RegistryError::DuplicateName(definition.name));
        }
        self.definitions.insert(definition.name.clone(), definition);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ToolDefinition> {
        self.definitions.get(name)
    }

    pub fn require(&self, name: &str) -> Result<&ToolDefinition, RegistryError> {
        self.get(name)
            .ok_or_else(|| RegistryError::UnknownTool(name.to_owned()))
    }

    pub fn contains(&self, name: &str) -> bool {
        self.definitions.contains_key(name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.definitions.keys().map(String::as_str).collect()
    }

    pub fn definitions(&self) -> Vec<&ToolDefinition> {
        self.definitions.values().collect()
    }

    /// 返回发送给 OpenAI-compatible Provider 的稳定 tool schema。
    pub fn model_schema(&self) -> Result<Value, RegistryError> {
        let definitions = self.definitions.values().cloned().collect::<Vec<_>>();
        serde_json::from_str(&canonical_tool_schema(&definitions)?)
            .map_err(|error| RegistryError::Serialization(error.to_string()))
    }

    /// 返回包含权限和资源限制的 generation fingerprint。
    pub fn schema_hash(&self) -> Result<String, RegistryError> {
        let definitions = self.definitions.values().cloned().collect::<Vec<_>>();
        tool_schema_hash(&definitions)
    }
}

#[cfg(test)]
mod tests {
    use super::ToolRegistry;
    use crate::{RegistryError, ToolDefinition, ToolPermission};
    use serde_json::json;

    fn definition(name: &str) -> ToolDefinition {
        ToolDefinition::new(
            name,
            format!("{name} tool"),
            json!({"type": "object", "properties": {}}),
            ToolPermission::ReadOnly,
            1,
            1024,
        )
        .unwrap()
    }

    #[test]
    fn registry_rejects_duplicate_without_changing_existing_definition() {
        let mut registry = ToolRegistry::new();
        registry.register(definition("read_file")).unwrap();
        let duplicate = ToolDefinition::new(
            "read_file",
            "replacement",
            json!({"type": "object"}),
            ToolPermission::Deny,
            2,
            2,
        )
        .unwrap();
        assert_eq!(
            registry.register(duplicate),
            Err(RegistryError::DuplicateName("read_file".into()))
        );
        assert_eq!(
            registry.get("read_file").unwrap().description,
            "read_file tool"
        );
    }

    #[test]
    fn lookup_and_names_are_stable() {
        let mut registry = ToolRegistry::new();
        registry.register(definition("terminal")).unwrap();
        registry.register(definition("read_file")).unwrap();
        assert!(registry.contains("read_file"));
        assert_eq!(registry.names(), vec!["read_file", "terminal"]);
        assert_eq!(
            registry.require("missing"),
            Err(RegistryError::UnknownTool("missing".into()))
        );
    }

    #[test]
    fn model_schema_is_a_json_array() {
        let mut registry = ToolRegistry::new();
        registry.register(definition("read_file")).unwrap();
        let schema = registry.model_schema().unwrap();
        assert!(schema.is_array());
        assert_eq!(schema[0]["function"]["name"], "read_file");
    }
}
