use sagent_tools::{RegistryError, ToolDefinition, ToolPermission, ToolRegistry};
use serde_json::json;

fn definition(name: &str, schema: serde_json::Value) -> ToolDefinition {
    ToolDefinition::new(
        name,
        format!("{name} fixture tool"),
        schema,
        ToolPermission::ReadOnly,
        30_000,
        32_768,
    )
    .expect("fixture 工具定义应有效")
}

#[test]
fn registration_order_does_not_change_provider_schema_or_hash() {
    let mut first = ToolRegistry::new();
    first
        .register(definition(
            "terminal",
            json!({"type": "object", "properties": {"command": {"type": "string"}}}),
        ))
        .unwrap();
    first
        .register(definition(
            "read_file",
            json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        ))
        .unwrap();

    let mut second = ToolRegistry::new();
    second
        .register(definition(
            "read_file",
            json!({"properties": {"path": {"type": "string"}}, "type": "object"}),
        ))
        .unwrap();
    second
        .register(definition(
            "terminal",
            json!({"properties": {"command": {"type": "string"}}, "type": "object"}),
        ))
        .unwrap();

    assert_eq!(
        first.model_schema().unwrap(),
        second.model_schema().unwrap()
    );
    assert_eq!(first.schema_hash().unwrap(), second.schema_hash().unwrap());
}

#[test]
fn schema_and_permission_changes_change_generation_hash() {
    let mut read = ToolRegistry::new();
    read.register(definition("read_file", json!({"type": "object"})))
        .unwrap();
    let read_hash = read.schema_hash().unwrap();

    let mut changed = ToolRegistry::new();
    changed
        .register(
            ToolDefinition::new(
                "read_file",
                "read_file fixture tool",
                json!({"type": "object", "required": ["path"]}),
                ToolPermission::ReadOnly,
                30_000,
                32_768,
            )
            .unwrap(),
        )
        .unwrap();
    assert_ne!(read_hash, changed.schema_hash().unwrap());

    let mut policy_changed = ToolRegistry::new();
    policy_changed
        .register(
            ToolDefinition::new(
                "read_file",
                "read_file fixture tool",
                json!({"type": "object"}),
                ToolPermission::ApprovalRequired,
                30_000,
                32_768,
            )
            .unwrap(),
        )
        .unwrap();
    assert_ne!(read_hash, policy_changed.schema_hash().unwrap());
}

#[test]
fn registry_rejects_duplicate_and_unknown_tools_without_secret_output() {
    let mut registry = ToolRegistry::new();
    let definition = ToolDefinition::new(
        "read_file",
        "读取文件",
        json!({"type": "object"}),
        ToolPermission::ReadOnly,
        1,
        1,
    )
    .unwrap();
    registry.register(definition.clone()).unwrap();
    assert_eq!(
        registry.register(definition),
        Err(RegistryError::DuplicateName("read_file".into()))
    );
    assert_eq!(
        registry.require("missing"),
        Err(RegistryError::UnknownTool("missing".into()))
    );
    assert!(!format!("{registry:?}").contains("OPENAI_API_KEY"));
}
