//! sagent-types 序列化/反序列化测试。
//!
//! 验证所有公共类型的 JSON round-trip、缺失必填字段的错误处理、
//! 以及 JSON 表示与协议决策一致。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 4 序列化测试

use sagent_types::envelope::Envelope;
use sagent_types::event::*;
use sagent_types::ids::*;
use sagent_types::message::*;
use sagent_types::session::*;
use sagent_types::tool::*;
use sagent_types::version::ProtocolVersion;

// ─── ID 类型测试 ───

#[test]
fn test_session_id_roundtrip() {
    let id = SessionId("sess_abc123".to_string());
    let json = serde_json::to_string(&id).unwrap();
    let parsed: SessionId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn test_session_id_json_format() {
    let id = SessionId("sess_abc123".to_string());
    let json = serde_json::to_string(&id).unwrap();
    // newtype 应序列化为内部字符串
    assert_eq!(json, r#""sess_abc123""#);
}

#[test]
fn test_turn_id_roundtrip() {
    let id = TurnId("turn_001".to_string());
    let json = serde_json::to_string(&id).unwrap();
    let parsed: TurnId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn test_message_id_roundtrip() {
    let id = MessageId("msg_001".to_string());
    let json = serde_json::to_string(&id).unwrap();
    let parsed: MessageId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn test_tool_call_id_roundtrip() {
    let id = ToolCallId("tc_001".to_string());
    let json = serde_json::to_string(&id).unwrap();
    let parsed: ToolCallId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn test_event_id_roundtrip() {
    let id = EventId("evt_001".to_string());
    let json = serde_json::to_string(&id).unwrap();
    let parsed: EventId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn test_request_id_string_roundtrip() {
    let id = RequestId::String("req-1".to_string());
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, r#""req-1""#);
    let parsed: RequestId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn test_request_id_number_roundtrip() {
    let id = RequestId::Number(42);
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, "42");
    let parsed: RequestId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn test_request_id_rejects_null() {
    // null 不是合法的 request ID
    let result: Result<RequestId, _> = serde_json::from_str("null");
    assert!(result.is_err());
}

#[test]
fn test_request_id_rejects_bool() {
    let result: Result<RequestId, _> = serde_json::from_str("true");
    assert!(result.is_err());
}

/// ID 类型互斥测试：不同类型 ID 不能互相传递。
/// 编译器保证 newtype 不能隐式转换。
#[test]
fn test_id_types_are_distinct() {
    let session = SessionId("sess_1".to_string());
    let turn = TurnId("turn_1".to_string());

    // 序列化后的 JSON 字符串不同（带类型前缀便于调试，但值相同说明互斥由类型保证）
    let session_json = serde_json::to_string(&session).unwrap();
    let turn_json = serde_json::to_string(&turn).unwrap();
    assert_ne!(session_json, turn_json);
}

// ─── Role 测试 ───

#[test]
fn test_role_serialization() {
    assert_eq!(serde_json::to_string(&Role::System).unwrap(), r#""system""#);
    assert_eq!(serde_json::to_string(&Role::User).unwrap(), r#""user""#);
    assert_eq!(
        serde_json::to_string(&Role::Assistant).unwrap(),
        r#""assistant""#
    );
    assert_eq!(serde_json::to_string(&Role::Tool).unwrap(), r#""tool""#);
}

#[test]
fn test_role_deserialization() {
    assert_eq!(
        serde_json::from_str::<Role>(r#""system""#).unwrap(),
        Role::System
    );
    assert_eq!(
        serde_json::from_str::<Role>(r#""user""#).unwrap(),
        Role::User
    );
    assert_eq!(
        serde_json::from_str::<Role>(r#""assistant""#).unwrap(),
        Role::Assistant
    );
    assert_eq!(
        serde_json::from_str::<Role>(r#""tool""#).unwrap(),
        Role::Tool
    );
}

#[test]
fn test_role_rejects_invalid() {
    assert!(serde_json::from_str::<Role>(r#""unknown""#).is_err());
    assert!(serde_json::from_str::<Role>(r#""function""#).is_err());
    assert!(serde_json::from_str::<Role>(r#""developer""#).is_err());
}

// ─── ContentPart 测试 ───

#[test]
fn test_content_part_text_roundtrip() {
    let part = ContentPart::Text {
        text: "hello world".to_string(),
    };
    let json = serde_json::to_string(&part).unwrap();
    let expected = r#"{"type":"text","text":"hello world"}"#;
    assert_eq!(json, expected);
    let parsed: ContentPart = serde_json::from_str(&json).unwrap();
    match parsed {
        ContentPart::Text { text } => assert_eq!(text, "hello world"),
    }
}

#[test]
fn test_content_part_rejects_unknown_type() {
    let result: Result<ContentPart, _> =
        serde_json::from_str(r#"{"type":"image","url":"http://example.com"}"#);
    assert!(result.is_err());
}

#[test]
fn test_content_part_rejects_missing_type() {
    let result: Result<ContentPart, _> = serde_json::from_str(r#"{"text":"hello"}"#);
    assert!(result.is_err());
}

// ─── Message 测试 ───

#[test]
fn test_message_user_roundtrip() {
    let msg = Message {
        message_id: MessageId("msg_1".to_string()),
        session_id: SessionId("sess_1".to_string()),
        role: Role::User,
        content: vec![ContentPart::Text {
            text: "Hello".to_string(),
        }],
        tool_calls: vec![],
        tool_call_id: None,
        created_at: "2026-08-07T12:00:00Z".to_string(),
        sequence: 1,
        metadata: Default::default(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: Message = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.message_id, MessageId("msg_1".to_string()));
    assert_eq!(parsed.role, Role::User);
    assert_eq!(parsed.content.len(), 1);
    assert!(parsed.tool_calls.is_empty());
    assert!(parsed.tool_call_id.is_none());
    assert_eq!(parsed.created_at, "2026-08-07T12:00:00Z");
}

#[test]
fn test_message_assistant_with_tool_calls() {
    let tc = ToolCall {
        id: ToolCallId("tc_1".to_string()),
        name: "read_file".to_string(),
        arguments: {
            let mut m = serde_json::Map::new();
            m.insert(
                "path".to_string(),
                serde_json::Value::String("/tmp/test.txt".to_string()),
            );
            m
        },
    };

    let msg = Message {
        message_id: MessageId("msg_2".to_string()),
        session_id: SessionId("sess_1".to_string()),
        role: Role::Assistant,
        content: vec![ContentPart::Text {
            text: "Let me read that file.".to_string(),
        }],
        tool_calls: vec![tc],
        tool_call_id: None,
        created_at: "2026-08-07T12:00:01Z".to_string(),
        sequence: 2,
        metadata: Default::default(),
    };

    let json = serde_json::to_string(&msg).unwrap();
    let parsed: Message = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.role, Role::Assistant);
    assert_eq!(parsed.tool_calls.len(), 1);
    assert_eq!(parsed.tool_calls[0].name, "read_file");
}

#[test]
fn test_message_tool_result() {
    let msg = Message {
        message_id: MessageId("msg_3".to_string()),
        session_id: SessionId("sess_1".to_string()),
        role: Role::Tool,
        content: vec![ContentPart::Text {
            text: "file contents here".to_string(),
        }],
        tool_calls: vec![],
        tool_call_id: Some(ToolCallId("tc_1".to_string())),
        created_at: "2026-08-07T12:00:02Z".to_string(),
        sequence: 3,
        metadata: Default::default(),
    };

    let json = serde_json::to_string(&msg).unwrap();
    let parsed: Message = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.role, Role::Tool);
    assert_eq!(parsed.tool_call_id, Some(ToolCallId("tc_1".to_string())));
}

#[test]
fn test_message_missing_message_id() {
    // 反序列化时缺少 message_id 应报错
    let json = r#"{
        "role": "user",
        "content": [{"type": "text", "text": "hello"}],
        "tool_calls": [],
        "created_at": "2026-08-07T12:00:00Z"
    }"#;
    let result: Result<Message, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn test_message_missing_session_id_or_content() {
    let missing_session_id = r#"{
        "message_id": "msg_1",
        "role": "user",
        "content": [{"type": "text", "text": "hello"}],
        "created_at": "2026-08-07T12:00:00Z",
        "sequence": 1
    }"#;
    assert!(serde_json::from_str::<Message>(missing_session_id).is_err());

    let missing_content = r#"{
        "message_id": "msg_1",
        "session_id": "sess_1",
        "role": "user",
        "created_at": "2026-08-07T12:00:00Z",
        "sequence": 1
    }"#;
    assert!(serde_json::from_str::<Message>(missing_content).is_err());
}

#[test]
fn test_message_missing_role() {
    let json = r#"{
        "message_id": "msg_1",
        "content": [{"type": "text", "text": "hello"}],
        "tool_calls": [],
        "created_at": "2026-08-07T12:00:00Z"
    }"#;
    let result: Result<Message, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn test_message_default_tool_calls_and_id() {
    // tool_calls 和 tool_call_id 应有默认值
    let json = r#"{
        "message_id": "msg_1",
        "session_id": "sess_1",
        "role": "user",
        "content": [{"type": "text", "text": "hello"}],
        "created_at": "2026-08-07T12:00:00Z",
        "sequence": 1
    }"#;
    let msg: Message = serde_json::from_str(json).unwrap();
    assert!(msg.tool_calls.is_empty());
    assert!(msg.tool_call_id.is_none());
}

// ─── ToolCall 测试 ───

#[test]
fn test_tool_call_roundtrip() {
    let tc = ToolCall {
        id: ToolCallId("tc_1".to_string()),
        name: "read_file".to_string(),
        arguments: {
            let mut m = serde_json::Map::new();
            m.insert(
                "path".to_string(),
                serde_json::Value::String("/tmp/test.txt".to_string()),
            );
            m
        },
    };

    let json = serde_json::to_string(&tc).unwrap();
    let parsed: ToolCall = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.id, ToolCallId("tc_1".to_string()));
    assert_eq!(parsed.name, "read_file");
    assert_eq!(
        parsed.arguments.get("path"),
        Some(&serde_json::Value::String("/tmp/test.txt".to_string()))
    );
}

#[test]
fn test_tool_call_missing_id() {
    let json = r#"{"name": "read_file", "arguments": {}}"#;
    let result: Result<ToolCall, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn test_tool_call_missing_name() {
    let json = r#"{"id": "tc_1", "arguments": {}}"#;
    let result: Result<ToolCall, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn test_tool_call_arguments_is_object() {
    // arguments 序列化后必须是 JSON object（Map）
    let tc = ToolCall {
        id: ToolCallId("tc_1".to_string()),
        name: "test".to_string(),
        arguments: {
            let mut m = serde_json::Map::new();
            m.insert("key".to_string(), serde_json::Value::Bool(true));
            m
        },
    };
    let json = serde_json::to_string(&tc).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(value["arguments"].is_object());
}

// ─── ToolDefinition 测试 ───

#[test]
fn test_tool_definition_roundtrip() {
    let td = ToolDefinition {
        name: "read_file".to_string(),
        description: "Read the contents of a file".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file"
                }
            },
            "required": ["path"]
        }),
    };

    let json = serde_json::to_string(&td).unwrap();
    let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.name, "read_file");
    assert_eq!(parsed.description, "Read the contents of a file");
    assert!(parsed.input_schema.is_object());
}

#[test]
fn test_tool_definition_missing_name() {
    let json = r#"{"description": "desc", "input_schema": {}}"#;
    let result: Result<ToolDefinition, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn test_tool_definition_minimal_fields() {
    // 只有三个字段：name, description, input_schema
    let td = ToolDefinition {
        name: "minimal".to_string(),
        description: "".to_string(),
        input_schema: serde_json::Value::Object(Default::default()),
    };
    let json = serde_json::to_string(&td).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let obj = value.as_object().unwrap();
    assert_eq!(obj.len(), 3);
    assert!(obj.contains_key("name"));
    assert!(obj.contains_key("description"));
    assert!(obj.contains_key("input_schema"));
}

// ─── ModelEvent 测试 ───

#[test]
fn test_model_event_message_delta_roundtrip() {
    let evt = ModelEvent::MessageDelta {
        delta: "hello".to_string(),
    };
    let json = serde_json::to_string(&evt).unwrap();
    let expected = r#"{"type":"message_delta","delta":"hello"}"#;
    assert_eq!(json, expected);

    let parsed: ModelEvent = serde_json::from_str(&json).unwrap();
    match parsed {
        ModelEvent::MessageDelta { delta } => assert_eq!(delta, "hello"),
        _ => panic!("expected MessageDelta"),
    }
}

#[test]
fn test_model_event_tool_start_roundtrip() {
    let evt = ModelEvent::ToolStart {
        name: "read_file".to_string(),
    };
    let json = serde_json::to_string(&evt).unwrap();
    let expected = r#"{"type":"tool_start","name":"read_file"}"#;
    assert_eq!(json, expected);

    let parsed: ModelEvent = serde_json::from_str(&json).unwrap();
    match parsed {
        ModelEvent::ToolStart { name } => assert_eq!(name, "read_file"),
        _ => panic!("expected ToolStart"),
    }
}

#[test]
fn test_model_event_tool_complete_roundtrip() {
    let evt = ModelEvent::ToolComplete {
        name: "read_file".to_string(),
        result: serde_json::json!({"content": "file contents"}),
    };
    let json = serde_json::to_string(&evt).unwrap();
    let parsed: ModelEvent = serde_json::from_str(&json).unwrap();
    match parsed {
        ModelEvent::ToolComplete { name, result } => {
            assert_eq!(name, "read_file");
            assert_eq!(result["content"], "file contents");
        },
        _ => panic!("expected ToolComplete"),
    }
}

#[test]
fn test_model_event_turn_completed_roundtrip() {
    let evt = ModelEvent::TurnCompleted {
        reason: "stop".to_string(),
    };
    let json = serde_json::to_string(&evt).unwrap();
    let expected = r#"{"type":"turn_completed","reason":"stop"}"#;
    assert_eq!(json, expected);

    let parsed: ModelEvent = serde_json::from_str(&json).unwrap();
    match parsed {
        ModelEvent::TurnCompleted { reason } => assert_eq!(reason, "stop"),
        _ => panic!("expected TurnCompleted"),
    }
}

#[test]
fn test_model_event_error_roundtrip() {
    let evt = ModelEvent::Error {
        code: -32603,
        message: "Internal error".to_string(),
    };
    let json = serde_json::to_string(&evt).unwrap();
    let parsed: ModelEvent = serde_json::from_str(&json).unwrap();
    match parsed {
        ModelEvent::Error { code, message } => {
            assert_eq!(code, -32603);
            assert_eq!(message, "Internal error");
        },
        _ => panic!("expected Error"),
    }
}

#[test]
fn test_model_event_rejects_unknown_type() {
    let result: Result<ModelEvent, _> =
        serde_json::from_str(r#"{"type":"unknown_event","data":"test"}"#);
    assert!(result.is_err());
}

#[test]
fn test_model_event_rejects_missing_type() {
    let result: Result<ModelEvent, _> = serde_json::from_str(r#"{"delta":"hello"}"#);
    assert!(result.is_err());
}

#[test]
fn test_model_event_all_variants_distinct() {
    // 每个 variant 的序列化 JSON 应不同
    let variants: Vec<(ModelEvent, &str)> = vec![
        (
            ModelEvent::MessageDelta {
                delta: "hi".to_string(),
            },
            "message_delta",
        ),
        (
            ModelEvent::ToolStart {
                name: "test".to_string(),
            },
            "tool_start",
        ),
        (
            ModelEvent::ToolComplete {
                name: "test".to_string(),
                result: serde_json::json!({}),
            },
            "tool_complete",
        ),
        (
            ModelEvent::TurnCompleted {
                reason: "stop".to_string(),
            },
            "turn_completed",
        ),
        (
            ModelEvent::Error {
                code: -1,
                message: "err".to_string(),
            },
            "error",
        ),
    ];

    for (evt, expected_type) in variants {
        let json = serde_json::to_string(&evt).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            value["type"].as_str().unwrap(),
            expected_type,
            "variant type mismatch"
        );
    }
}

// ─── Envelope 测试 ───

#[test]
fn test_envelope_roundtrip() {
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct TestPayload {
        value: String,
    }

    let env: Envelope<TestPayload> = Envelope {
        protocol: "sagent.rpc".to_string(),
        version: 1,
        data: TestPayload {
            value: "test".to_string(),
        },
    };

    let json = serde_json::to_string(&env).unwrap();
    let parsed: Envelope<TestPayload> = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.protocol, "sagent.rpc");
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.data.value, "test");
}

use serde::{Deserialize, Serialize};

#[test]
fn test_envelope_with_message() {
    let msg = Message {
        message_id: MessageId("msg_1".to_string()),
        session_id: SessionId("sess_1".to_string()),
        role: Role::System,
        content: vec![ContentPart::Text {
            text: "system prompt".to_string(),
        }],
        tool_calls: vec![],
        tool_call_id: None,
        created_at: "2026-08-07T12:00:00Z".to_string(),
        sequence: 1,
        metadata: Default::default(),
    };

    let env: Envelope<Message> = Envelope {
        protocol: "sagent.rpc".to_string(),
        version: 1,
        data: msg,
    };

    let json = serde_json::to_string(&env).unwrap();
    let parsed: Envelope<Message> = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.data.role, Role::System);
    assert_eq!(parsed.protocol, "sagent.rpc");
}

// ─── ProtocolVersion 测试 ───

#[test]
fn test_protocol_version_roundtrip() {
    let pv = ProtocolVersion {
        protocol: "sagent.rpc".to_string(),
        version: 1,
        runtime_version: "0.1.0".to_string(),
        features: vec![
            "rpc.echo".to_string(),
            "protocol.describe".to_string(),
            "health.get".to_string(),
        ],
    };

    let json = serde_json::to_string(&pv).unwrap();
    let parsed: ProtocolVersion = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.protocol, "sagent.rpc");
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.runtime_version, "0.1.0");
    assert_eq!(parsed.features.len(), 3);
}

#[test]
fn test_protocol_version_default() {
    let pv = ProtocolVersion::default();
    assert_eq!(pv.protocol, "sagent.rpc");
    assert_eq!(pv.version, 1);
    assert!(!pv.runtime_version.is_empty());
    assert!(pv.features.contains(&"rpc.echo".to_string()));
    assert!(pv.features.contains(&"protocol.describe".to_string()));
    assert!(pv.features.contains(&"health.get".to_string()));
}

#[test]
fn test_protocol_version_missing_features() {
    // features 缺失应报错
    let json = r#"{
        "protocol": "sagent.rpc",
        "version": 1,
        "runtime_version": "0.1.0"
    }"#;
    let result: Result<ProtocolVersion, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

// ─── 跨类型序列化一致性测试 ───

#[test]
fn test_timestamp_is_rfc3339_string() {
    // 确保时间戳字段是 RFC 3339 字符串格式
    let msg = Message {
        message_id: MessageId("m1".to_string()),
        session_id: SessionId("sess_1".to_string()),
        role: Role::System,
        content: vec![ContentPart::Text {
            text: "test".to_string(),
        }],
        tool_calls: vec![],
        tool_call_id: None,
        created_at: "2026-08-07T12:00:00Z".to_string(),
        sequence: 1,
        metadata: Default::default(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let created_at = value["created_at"].as_str().unwrap();

    // RFC 3339 特征：含 T 分隔符和 Z 后缀
    assert!(created_at.contains('T'));
    assert!(created_at.ends_with('Z'));
}

#[test]
fn test_message_content_is_always_array() {
    // content 序列化后始终是 JSON 数组
    let msg = Message {
        message_id: MessageId("m1".to_string()),
        session_id: SessionId("sess_1".to_string()),
        role: Role::User,
        content: vec![ContentPart::Text {
            text: "hi".to_string(),
        }],
        tool_calls: vec![],
        tool_call_id: None,
        created_at: "2026-08-07T12:00:00Z".to_string(),
        sequence: 1,
        metadata: Default::default(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(value["content"].is_array());
}

#[test]
fn test_no_panic_on_invalid_json() {
    // 非法 JSON 应返回错误，不 panic
    let result: Result<Message, _> = serde_json::from_str("not json at all");
    assert!(result.is_err());

    let result: Result<Message, _> = serde_json::from_str("42");
    assert!(result.is_err());

    let result: Result<Message, _> = serde_json::from_str("null");
    assert!(result.is_err());
}

// ─── Fixture 一致性测试 ───

#[test]
fn test_fixture_message_matches_derive() {
    // 验证 fixture 文件与 Rust derive 输出一致
    let fixture: Message = serde_json::from_str(include_str!(
        "../../../protocols/fixtures/valid/message.json"
    ))
    .unwrap();

    assert_eq!(fixture.role, Role::User);
    assert_eq!(fixture.session_id, SessionId("sess_1".to_string()));
    assert_eq!(fixture.sequence, 1);
    assert_eq!(fixture.content.len(), 1);
    assert!(fixture.tool_calls.is_empty());
    assert!(fixture.tool_call_id.is_none());

    // 反序列化后再序列化，应与 fixture 一致
    let re_json = serde_json::to_string_pretty(&fixture).unwrap();
    let re_parsed: Message = serde_json::from_str(&re_json).unwrap();
    assert_eq!(fixture.message_id, re_parsed.message_id);
    assert_eq!(fixture.role, re_parsed.role);
}

#[test]
fn test_session_roundtrip_and_projection_invariants() {
    let session = Session {
        id: SessionId("sess_1".to_string()),
        source: "stdio".to_string(),
        title: Some("Test session".to_string()),
        created_at: "2026-08-07T12:00:00Z".to_string(),
        updated_at: "2026-08-07T12:00:00Z".to_string(),
        status: SessionStatus::Active,
        cwd: Some("/tmp/workspace".to_string()),
        metadata: serde_json::json!({"kind": "test"}).as_object().expect("object").clone(),
        message_count: 0,
        revision: 0,
    };
    session.validate().expect("合法 Session 应通过校验");
    let json = serde_json::to_string(&session).expect("Session 应可序列化");
    let parsed: Session = serde_json::from_str(&json).expect("Session 应可反序列化");
    assert_eq!(parsed.id, session.id);
    assert_eq!(parsed.status, SessionStatus::Active);
    assert_eq!(parsed.message_count, 0);
    assert_eq!(parsed.revision, 0);

    let after_message = session.after_message_commit("2026-08-07T12:00:01Z".to_string());
    assert_eq!(after_message.message_count, 1);
    assert_eq!(after_message.revision, 1);
    assert_eq!(after_message.status, SessionStatus::Active);

    let after_close = after_message.after_close_commit("2026-08-07T12:00:02Z".to_string());
    assert_eq!(after_close.message_count, 1);
    assert_eq!(after_close.revision, 2);
    assert_eq!(after_close.status, SessionStatus::Closed);
}

#[test]
fn test_persisted_message_validation_requires_session_and_sequence() {
    let message = Message {
        message_id: MessageId("msg_1".to_string()),
        session_id: SessionId("sess_1".to_string()),
        role: Role::User,
        content: vec![ContentPart::Text {
            text: "hello".to_string(),
        }],
        tool_calls: vec![],
        tool_call_id: None,
        created_at: "2026-08-07T12:00:00Z".to_string(),
        sequence: 1,
        metadata: Default::default(),
    };
    message.validate().expect("合法消息应通过校验");

    let mut invalid = message.clone();
    invalid.session_id = SessionId(String::new());
    assert_eq!(
        invalid.validate(),
        Err(MessageValidationError::EmptySessionId)
    );
    invalid.session_id = SessionId("sess_1".to_string());
    invalid.sequence = 0;
    assert_eq!(
        invalid.validate(),
        Err(MessageValidationError::InvalidSequence)
    );
}

#[test]
fn test_session_status_rejects_agent_execution_states() {
    assert!(serde_json::from_str::<SessionStatus>(r#""active""#).is_ok());
    assert!(serde_json::from_str::<SessionStatus>(r#""closed""#).is_ok());
    assert!(serde_json::from_str::<SessionStatus>(r#""recovering""#).is_ok());
    assert!(serde_json::from_str::<SessionStatus>(r#""thinking""#).is_err());
}

#[test]
fn test_session_fixture_matches_derive() {
    let fixture: Session = serde_json::from_str(include_str!(
        "../../../protocols/fixtures/valid/session.json"
    ))
    .expect("Session fixture 应可反序列化");
    fixture.validate().expect("fixture 应满足 Session 不变量");
    assert_eq!(fixture.id, SessionId("sess_1".to_string()));
    assert_eq!(fixture.status, SessionStatus::Active);
    assert_eq!(fixture.message_count, 1);
    assert_eq!(fixture.revision, 1);
}

#[test]
fn test_fixture_tool_call_matches_derive() {
    let fixture: ToolCall = serde_json::from_str(include_str!(
        "../../../protocols/fixtures/valid/tool-call.json"
    ))
    .unwrap();

    assert_eq!(fixture.name, "read_file");
    assert_eq!(
        fixture.arguments.get("path"),
        Some(&serde_json::Value::String("/tmp/test.txt".to_string()))
    );

    let re_json = serde_json::to_string(&fixture).unwrap();
    let re_parsed: ToolCall = serde_json::from_str(&re_json).unwrap();
    assert_eq!(fixture.id, re_parsed.id);
    assert_eq!(fixture.name, re_parsed.name);
}

#[test]
fn test_fixture_tool_definition_matches_derive() {
    let fixture: ToolDefinition = serde_json::from_str(include_str!(
        "../../../protocols/fixtures/valid/tool-definition.json"
    ))
    .unwrap();

    assert_eq!(fixture.name, "read_file");
    assert!(fixture.description.contains("Read"));
    assert!(fixture.input_schema.is_object());

    // 三个字段，不多不少
    let json = serde_json::to_string(&fixture).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 3);
}

#[test]
fn test_fixture_model_event_matches_derive() {
    let fixture: ModelEvent = serde_json::from_str(include_str!(
        "../../../protocols/fixtures/valid/model-event.json"
    ))
    .unwrap();

    match fixture {
        ModelEvent::MessageDelta { delta } => assert_eq!(delta, "Hello, world!"),
        _ => panic!("expected MessageDelta"),
    }
}
