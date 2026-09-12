//! 工具定义注册表。

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{
    RegistryError, ToolDefinition, ToolPermission, canonical_tool_schema, tool_schema_hash,
};

/// 保存当前 generation 可见的工具定义。
///
/// 使用 BTreeMap 保证查询和遍历本身稳定；schema helper 仍会再次按名称排序，
/// 从而不依赖调用方传入顺序。
#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    definitions: BTreeMap<String, ToolDefinition>,
}

impl ToolRegistry {
    /// 创建空注册表。
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

    /// 按名称查找工具，不改变注册表。
    pub fn get(&self, name: &str) -> Option<&ToolDefinition> {
        self.definitions.get(name)
    }

    /// 按名称查找工具，找不到时返回稳定错误。
    pub fn require(&self, name: &str) -> Result<&ToolDefinition, RegistryError> {
        self.get(name)
            .ok_or_else(|| RegistryError::UnknownTool(name.to_owned()))
    }

    /// 判断注册表是否包含指定工具。
    pub fn contains(&self, name: &str) -> bool {
        self.definitions.contains_key(name)
    }

    /// 返回按字典序排列的工具名。
    pub fn names(&self) -> Vec<&str> {
        self.definitions.keys().map(String::as_str).collect()
    }

    /// 返回按名称稳定排列的工具定义引用。
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

/// 返回 Runtime 默认允许暴露给 Provider 的最小工具集合。
///
/// Registry 是唯一的工具元数据来源；RPC bootstrap 与测试都复用这组定义，避免模型
/// schema、权限级别和执行器支持的工具名称发生漂移。具体 workspace、Store 和审批状态
/// 仍由上层注入，不能在这里绑定用户路径或执行副作用。
pub fn builtin_registry() -> Result<ToolRegistry, RegistryError> {
    use serde_json::json;

    let definitions = [
        ToolDefinition::new(
            "read_file",
            "Read a file",
            json!({
                "properties": {"path": {"type": "string"}},
                "type": "object",
                "required": ["path"]
            }),
            ToolPermission::ReadOnly,
            30_000,
            4_096,
        )?,
        ToolDefinition::new(
            "session_search",
            "Search conversation messages in the current Profile",
            json!({
                "additionalProperties": false,
                "properties": {
                    "limit": {"minimum": 1, "type": "integer"},
                    "query": {"type": "string"},
                    "session_id": {"type": "string"}
                },
                "type": "object",
                "required": ["query"]
            }),
            ToolPermission::ReadOnly,
            30_000,
            16_384,
        )?,
        ToolDefinition::new(
            "terminal",
            "Run a command",
            json!({
                "properties": {"command": {"type": "string"}},
                "type": "object",
                "required": ["command"]
            }),
            ToolPermission::ApprovalRequired,
            30_000,
            4_096,
        )?,
        ToolDefinition::new(
            "write_file",
            "Write a workspace text file atomically",
            json!({
                "additionalProperties": false,
                "properties": {
                    "content": {"type": "string"},
                    "overwrite": {"type": "boolean"},
                    "path": {"type": "string"}
                },
                "type": "object",
                "required": ["path", "content"]
            }),
            ToolPermission::ApprovalRequired,
            30_000,
            4_096,
        )?,
    ];
    let mut registry = ToolRegistry::new();
    for definition in definitions {
        registry.register(definition)?;
    }
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::{ToolRegistry, builtin_registry};
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

    #[test]
    fn builtin_registry_keeps_the_contract_tool_set_and_order() {
        let registry = builtin_registry().expect("默认工具定义必须有效");

        assert_eq!(
            registry.names(),
            vec!["read_file", "session_search", "terminal", "write_file"]
        );
        assert_eq!(
            registry
                .get("terminal")
                .expect("terminal 应注册")
                .permission,
            ToolPermission::ApprovalRequired
        );
    }
}
