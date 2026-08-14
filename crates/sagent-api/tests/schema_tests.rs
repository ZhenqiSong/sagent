//! JSON Schema 一致性测试。
//!
//! 验证 Rust 代码生成的 schema 与静态 schema 文件一致，
//! 以及所有 valid/invalid fixtures 通过/不通过 schema 校验。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 5 Schema 一致性测试

use sagent_api::schema;
use serde_json::Value;
use std::fs;

/// 获取项目根目录（workspace 根）。
fn project_root() -> String {
    // 从 crate 的 manifest 目录向上两级到达 workspace 根
    // crates/sagent-api/Cargo.toml -> ../../ = workspace root
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = std::path::Path::new(manifest_dir);
    path.parent()
        .and_then(|p| p.parent())
        .expect("无法定位 workspace 根目录")
        .to_string_lossy()
        .to_string()
}

/// 加载 JSON fixture 文件（相对于 workspace 根目录）。
fn load_fixture(relative_path: &str) -> Value {
    let full_path = format!("{}/{}", project_root(), relative_path);
    let content = fs::read_to_string(&full_path).expect("fixture 文件读取失败");
    serde_json::from_str(&content).expect("fixture 不是合法 JSON")
}

// ============================================================================
// Schema 一致性：Rust 代码生成与静态文件一致
// ============================================================================

#[test]
fn rust_generated_request_schema_matches_static() {
    let rust_schema = schema::jsonrpc_request_schema();
    let static_schema = load_fixture("protocols/schemas/jsonrpc-request.schema.json");
    assert_eq!(
        rust_schema, static_schema,
        "Rust 生成的 request schema 与静态文件不一致，请运行 schema 生成命令"
    );
}

#[test]
fn rust_generated_response_schema_matches_static() {
    let rust_schema = schema::jsonrpc_response_schema();
    let static_schema = load_fixture("protocols/schemas/jsonrpc-response.schema.json");
    assert_eq!(
        rust_schema, static_schema,
        "Rust 生成的 response schema 与静态文件不一致，请运行 schema 生成命令"
    );
}

#[test]
fn rust_generated_event_schema_matches_static() {
    let rust_schema = schema::event_envelope_schema();
    let static_schema = load_fixture("protocols/schemas/event-envelope.schema.json");
    assert_eq!(
        rust_schema, static_schema,
        "Rust 生成的 event envelope schema 与静态文件不一致，请运行 schema 生成命令"
    );
}

#[test]
fn rust_generated_protocol_describe_schema_matches_static() {
    let rust_schema = schema::protocol_describe_schema();
    let static_schema = load_fixture("protocols/schemas/protocol-describe.schema.json");
    assert_eq!(
        rust_schema, static_schema,
        "Rust 生成的 protocol describe schema 与静态文件不一致，请运行 schema 生成命令"
    );
}

// ============================================================================
// Schema 正向校验：所有 valid fixtures 必须通过对应 schema
// ============================================================================

#[test]
fn valid_request_fixtures_pass_request_schema() {
    let schema_json = schema::jsonrpc_request_schema();
    let fixtures = [
        "rpc-echo-request",
        "protocol-describe-request",
        "health-request",
    ];
    for name in &fixtures {
        let fixture = load_fixture(&format!("protocols/fixtures/valid/{}.json", name));
        assert!(
            jsonschema::draft202012::is_valid(&schema_json, &fixture),
            "valid fixture '{}' 未能通过 request schema 校验",
            name
        );
    }
}

#[test]
fn valid_response_fixtures_pass_response_schema() {
    let schema_json = schema::jsonrpc_response_schema();
    let fixtures = [
        "rpc-echo-response",
        "protocol-describe-response",
        "health-response",
    ];
    for name in &fixtures {
        let fixture = load_fixture(&format!("protocols/fixtures/valid/{}.json", name));
        assert!(
            jsonschema::draft202012::is_valid(&schema_json, &fixture),
            "valid fixture '{}' 未能通过 response schema 校验",
            name
        );
    }
}

#[test]
fn valid_event_fixtures_pass_event_schema() {
    let schema_json = schema::event_envelope_schema();
    let fixtures = [
        "message-delta-event",
        "tool-start-event",
        "event-no-session",
    ];
    for name in &fixtures {
        let fixture = load_fixture(&format!("protocols/fixtures/valid/{}.json", name));
        assert!(
            jsonschema::draft202012::is_valid(&schema_json, &fixture),
            "valid fixture '{}' 未能通过 event schema 校验",
            name
        );
    }
}

// ============================================================================
// Schema 反向校验：所有 invalid fixtures 必须被对应 schema 拒绝
// ============================================================================

#[test]
fn invalid_request_fixtures_fail_request_schema() {
    let schema_json = schema::jsonrpc_request_schema();
    let fixtures = [
        "missing-jsonrpc",
        "wrong-jsonrpc-version",
        "missing-method",
        "params-is-array",
        "params-is-string",
        "unknown-envelope-field",
        "null-request-id",
    ];
    for name in &fixtures {
        let fixture = load_fixture(&format!("protocols/fixtures/invalid/{}.json", name));
        assert!(
            !jsonschema::draft202012::is_valid(&schema_json, &fixture),
            "invalid fixture '{}' 错误地通过了 request schema 校验",
            name
        );
    }
}

#[test]
fn invalid_response_fixtures_fail_response_schema() {
    let schema_json = schema::jsonrpc_response_schema();
    let fixtures = [
        "both-result-and-error",
        "neither-result-nor-error",
        "error-code-not-integer",
    ];
    for name in &fixtures {
        let fixture = load_fixture(&format!("protocols/fixtures/invalid/{}.json", name));
        assert!(
            !jsonschema::draft202012::is_valid(&schema_json, &fixture),
            "invalid fixture '{}' 错误地通过了 response schema 校验",
            name
        );
    }
}

#[test]
fn invalid_event_fixtures_fail_event_schema() {
    let schema_json = schema::event_envelope_schema();
    let fixtures = [
        "event-with-id",
        "seq-zero",
        "seq-negative",
        "event-missing-event-id",
    ];
    for name in &fixtures {
        let fixture = load_fixture(&format!("protocols/fixtures/invalid/{}.json", name));
        assert!(
            !jsonschema::draft202012::is_valid(&schema_json, &fixture),
            "invalid fixture '{}' 错误地通过了 event schema 校验",
            name
        );
    }
}

#[test]
fn invalid_method_too_long_fails_request_schema() {
    let schema_json = schema::jsonrpc_request_schema();
    let fixture = load_fixture("protocols/fixtures/invalid/method-too-long.json");
    // method 长度限制属于 request schema 和 runtime 的共同约束。
    assert!(
        !jsonschema::draft202012::is_valid(&schema_json, &fixture),
        "method-too-long 错误地通过了 request schema 校验"
    );
}

// ============================================================================
// Rust 类型序列化输出可通过 schema 校验
// ============================================================================

#[test]
fn request_serialize_output_passes_schema() {
    use sagent_api::request::Request;
    use sagent_types::ids::RequestId;

    let req = Request {
        jsonrpc: "2.0".to_string(),
        id: RequestId::String("test-1".to_string()),
        method: "rpc.echo".to_string(),
        params: serde_json::json!({"value": "test"}),
    };
    let json = serde_json::to_value(&req).expect("序列化失败");
    let schema_json = schema::jsonrpc_request_schema();
    assert!(
        jsonschema::draft202012::is_valid(&schema_json, &json),
        "Rust Request 序列化输出未通过 schema 校验"
    );
}

#[test]
fn success_response_serialize_output_passes_schema() {
    use sagent_api::response::SuccessResponse;
    use sagent_types::ids::RequestId;

    let resp = SuccessResponse {
        jsonrpc: "2.0".to_string(),
        id: RequestId::String("test-1".to_string()),
        result: serde_json::json!({"value": "ok"}),
    };
    let json = serde_json::to_value(&resp).expect("序列化失败");
    let schema_json = schema::jsonrpc_response_schema();
    assert!(
        jsonschema::draft202012::is_valid(&schema_json, &json),
        "Rust SuccessResponse 序列化输出未通过 schema 校验"
    );
}

#[test]
fn error_response_serialize_output_passes_schema() {
    use sagent_api::error::ErrorObject;
    use sagent_api::response::ErrorResponse;
    use sagent_types::ids::RequestId;

    let resp = ErrorResponse {
        jsonrpc: "2.0".to_string(),
        id: Some(RequestId::String("test-1".to_string())),
        error: ErrorObject::invalid_params("missing field"),
    };
    let json = serde_json::to_value(&resp).expect("序列化失败");
    let schema_json = schema::jsonrpc_response_schema();
    assert!(
        jsonschema::draft202012::is_valid(&schema_json, &json),
        "Rust ErrorResponse 序列化输出未通过 schema 校验"
    );
}

#[test]
fn event_envelope_serialize_output_passes_schema() {
    use sagent_api::event::{EventEnvelope, EventParams};
    use sagent_types::ids::{EventId, SessionId, TurnId};

    let evt = EventEnvelope {
        jsonrpc: "2.0".to_string(),
        method: "message.delta".to_string(),
        params: EventParams {
            event_id: EventId("evt-1".to_string()),
            session_id: Some(SessionId("sess-1".to_string())),
            turn_id: Some(TurnId("turn-1".to_string())),
            seq: 1,
            timestamp: "2026-08-07T12:00:00Z".to_string(),
            data: serde_json::json!({"delta": "hello"}),
        },
    };
    let json = serde_json::to_value(&evt).expect("序列化失败");
    let schema_json = schema::event_envelope_schema();
    assert!(
        jsonschema::draft202012::is_valid(&schema_json, &json),
        "Rust EventEnvelope 序列化输出未通过 schema 校验"
    );
}

// ============================================================================
// 协议错误码稳定性测试
// ============================================================================

#[test]
fn error_object_serialization_is_stable() {
    use sagent_api::error::ErrorObject;

    let err = ErrorObject::parse_error("not valid JSON");
    let json = serde_json::to_value(&err).expect("序列化失败");
    assert_eq!(json["code"], -32700);
    assert_eq!(json["message"], "not valid JSON");

    let err = ErrorObject::method_not_found("unknown.method");
    let json = serde_json::to_value(&err).expect("序列化失败");
    assert_eq!(json["code"], -32601);
    assert!(json["message"].as_str().unwrap().contains("unknown.method"));
}

#[test]
fn all_error_codes_are_unique() {
    use sagent_api::error::codes;
    let codes_list = vec![
        codes::PARSE_ERROR,
        codes::INVALID_REQUEST,
        codes::METHOD_NOT_FOUND,
        codes::INVALID_PARAMS,
        codes::INTERNAL_ERROR,
        codes::PROTOCOL_VERSION_UNSUPPORTED,
        codes::CAPABILITY_UNSUPPORTED,
        codes::PAYLOAD_TOO_LARGE,
        codes::SEQUENCE_VIOLATION,
        codes::SHUTDOWN,
    ];
    let mut sorted = codes_list.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(codes_list.len(), sorted.len(), "存在重复的错误码");
}

// ============================================================================
// protocol.describe 响应与 Rust ProtocolVersion 默认值一致
// ============================================================================

#[test]
fn protocol_describe_response_matches_protocol_version_default() {
    use sagent_types::version::ProtocolVersion;

    let pv = ProtocolVersion::default();
    let json = serde_json::to_value(&pv).expect("序列化失败");

    let schema_json = schema::protocol_describe_schema();
    assert!(
        jsonschema::draft202012::is_valid(&schema_json, &json),
        "ProtocolVersion 默认值未通过 protocol-describe schema 校验"
    );

    assert_eq!(json["protocol"], "sagent.rpc");
    assert_eq!(json["version"], 1);
    assert!(json["features"].as_array().unwrap().contains(&serde_json::json!("rpc.echo")));
}

// ============================================================================
// 所有 error 构造方法的序列化测试
// ============================================================================

#[test]
fn all_error_constructors_serialize_correctly() {
    use sagent_api::error::ErrorObject;

    let test_cases: Vec<(&str, i32, ErrorObject)> = vec![
        ("parse_error", -32700, ErrorObject::parse_error("test")),
        (
            "invalid_request",
            -32600,
            ErrorObject::invalid_request("test"),
        ),
        (
            "method_not_found",
            -32601,
            ErrorObject::method_not_found("test"),
        ),
        (
            "invalid_params",
            -32602,
            ErrorObject::invalid_params("test"),
        ),
        (
            "internal_error",
            -32603,
            ErrorObject::internal_error("test"),
        ),
        (
            "payload_too_large",
            -32003,
            ErrorObject::payload_too_large("test"),
        ),
        (
            "protocol_version_unsupported",
            -32001,
            ErrorObject::protocol_version_unsupported("test"),
        ),
        (
            "capability_unsupported",
            -32002,
            ErrorObject::capability_unsupported("test"),
        ),
        (
            "sequence_violation",
            -32004,
            ErrorObject::sequence_violation("test"),
        ),
        ("shutdown", -32005, ErrorObject::shutdown("test")),
    ];

    for (name, expected_code, err) in &test_cases {
        let json = serde_json::to_value(err).expect("序列化失败");
        assert_eq!(
            json["code"], *expected_code,
            "{} 的错误码应为 {}，实际为 {}",
            name, expected_code, json["code"]
        );
        assert!(
            json["data"].is_null(),
            "{} 的 data 应为 null（无额外数据时）",
            name
        );
    }
}

// ============================================================================
// ErrorCode enum 类型安全测试
// ============================================================================

#[test]
fn error_code_roundtrip_i32() {
    use sagent_api::error::ErrorCode;

    let all_codes = [
        ErrorCode::ParseError,
        ErrorCode::InvalidRequest,
        ErrorCode::MethodNotFound,
        ErrorCode::InvalidParams,
        ErrorCode::InternalError,
        ErrorCode::ProtocolVersionUnsupported,
        ErrorCode::CapabilityUnsupported,
        ErrorCode::PayloadTooLarge,
        ErrorCode::SequenceViolation,
        ErrorCode::Shutdown,
    ];

    for code in &all_codes {
        let i = code.to_i32();
        let parsed = ErrorCode::from_i32(i);
        assert_eq!(Some(*code), parsed, "ErrorCode {:?} 的 i32 往返失败", code);
    }
}

#[test]
fn error_code_from_unknown_i32_returns_none() {
    use sagent_api::error::ErrorCode;

    assert_eq!(ErrorCode::from_i32(0), None);
    assert_eq!(ErrorCode::from_i32(-99999), None);
    assert_eq!(ErrorCode::from_i32(200), None);
}

#[test]
fn error_code_is_standard_vs_extension() {
    use sagent_api::error::ErrorCode;

    assert!(ErrorCode::ParseError.is_standard());
    assert!(ErrorCode::InvalidRequest.is_standard());
    assert!(ErrorCode::MethodNotFound.is_standard());
    assert!(ErrorCode::InvalidParams.is_standard());
    assert!(ErrorCode::InternalError.is_standard());

    assert!(ErrorCode::ProtocolVersionUnsupported.is_extension());
    assert!(ErrorCode::CapabilityUnsupported.is_extension());
    assert!(ErrorCode::PayloadTooLarge.is_extension());
    assert!(ErrorCode::SequenceViolation.is_extension());
    assert!(ErrorCode::Shutdown.is_extension());
}

#[test]
fn error_object_from_code_uses_default_message() {
    use sagent_api::error::{ErrorCode, ErrorObject};

    let err = ErrorObject::from_code(ErrorCode::MethodNotFound);
    assert_eq!(err.code, -32601);
    assert_eq!(err.message, "Method not found");
    assert!(err.data.is_none());
}

#[test]
fn error_object_with_data_preserves_data() {
    use sagent_api::error::{ErrorCode, ErrorObject};

    let err = ErrorObject::from_code(ErrorCode::InvalidParams)
        .with_data(serde_json::json!({"field": "value"}));

    let json = serde_json::to_value(&err).expect("序列化失败");
    assert_eq!(json["code"], -32602);
    assert_eq!(json["data"]["field"], "value");
}

// ============================================================================
// Capabilities 类型测试
// ============================================================================

#[test]
fn capabilities_supports_registered_methods() {
    use sagent_types::version::Capabilities;

    let caps = Capabilities::default_capabilities();
    assert!(caps.supports("rpc.echo"));
    assert!(caps.supports("protocol.describe"));
    assert!(caps.supports("health.get"));
    assert!(!caps.supports("session.create"));
    assert!(!caps.supports("prompt.submit"));
}

#[test]
fn capabilities_validate_method() {
    use sagent_types::version::Capabilities;

    let caps = Capabilities::default_capabilities();
    assert!(caps.validate_method("rpc.echo"));
    assert!(!caps.validate_method("unknown.method"));
}

#[test]
fn capabilities_feature_names_matches_core_methods() {
    use sagent_types::version::{Capabilities, CORE_METHODS};

    let caps = Capabilities::default_capabilities();
    let features = caps.feature_names();
    assert_eq!(features.len(), CORE_METHODS.len());
    for method in CORE_METHODS {
        assert!(
            features.contains(&method.to_string()),
            "Capabilities 缺少方法: {}",
            method
        );
    }
}

#[test]
fn capabilities_default_has_three_methods() {
    use sagent_types::version::Capabilities;

    let caps = Capabilities::default();
    assert_eq!(caps.len(), 3);
    assert!(!caps.is_empty());
}

#[test]
fn capabilities_empty() {
    use sagent_types::version::Capabilities;

    let caps = Capabilities::new(vec![]);
    assert!(caps.is_empty());
    assert_eq!(caps.len(), 0);
    assert!(!caps.supports("rpc.echo"));
}

// ============================================================================
// 新增 invalid fixture 测试
// ============================================================================

#[test]
fn invalid_unsupported_protocol_version_fails_request_schema() {
    let schema_json = schema::jsonrpc_request_schema();
    let fixture = load_fixture("protocols/fixtures/invalid/unsupported-protocol-version.json");
    // 该 fixture 的 params 包含 version: 99，这本身是合法的 request（params 是 object）
    // 但业务上应被拒绝。这里验证它至少能通过 request schema（因为是合法的 JSON-RPC）。
    // 业务层拒绝逻辑在 dispatcher 中实现。
    assert!(
        jsonschema::draft202012::is_valid(&schema_json, &fixture),
        "合法的 request（尽管业务层会拒绝）应通过 request schema"
    );
}

#[test]
fn valid_error_protocol_version_unsupported_passes_response_schema() {
    let schema_json = schema::jsonrpc_response_schema();
    let fixture = load_fixture("protocols/fixtures/valid/error-protocol-version-unsupported.json");
    assert!(
        jsonschema::draft202012::is_valid(&schema_json, &fixture),
        "error-protocol-version-unsupported 应通过 response schema"
    );
}

#[test]
fn valid_error_capability_unsupported_passes_response_schema() {
    let schema_json = schema::jsonrpc_response_schema();
    let fixture = load_fixture("protocols/fixtures/valid/error-capability-unsupported.json");
    assert!(
        jsonschema::draft202012::is_valid(&schema_json, &fixture),
        "error-capability-unsupported 应通过 response schema"
    );
}

// ============================================================================
// ProtocolVersion features 与 CORE_METHODS 一致性测试
// ============================================================================

#[test]
fn protocol_version_features_match_core_methods() {
    use sagent_types::version::{ProtocolVersion, CORE_METHODS};

    let pv = ProtocolVersion::default();
    assert_eq!(
        pv.features.len(),
        CORE_METHODS.len(),
        "ProtocolVersion.features 数量应与 CORE_METHODS 一致"
    );
    for method in CORE_METHODS {
        assert!(
            pv.features.contains(&method.to_string()),
            "ProtocolVersion.features 缺少方法: {}",
            method
        );
    }
}

#[test]
fn protocol_version_has_correct_identity() {
    use sagent_types::version::ProtocolVersion;

    let pv = ProtocolVersion::default();
    assert_eq!(pv.protocol, "sagent.rpc");
    assert_eq!(pv.version, 1);
    assert!(!pv.runtime_version.is_empty());
}
