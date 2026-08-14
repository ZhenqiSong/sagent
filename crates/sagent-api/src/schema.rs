//! Schema 生成和校验模块。
//!
//! Phase 0 提供 JSON-RPC 2.0 协议 schema 的代码生成入口，
//! 所有 schema 必须与 `protocols/schemas/` 下的静态文件保持一致。
//! 修改 Rust 类型后应重新生成 schema 文件并确保无 diff。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 JSON Schema 生成

/// 生成 JSON-RPC request 的 JSON Schema。
///
/// 约束：jsonrpc 必须为 "2.0"；id 为 string 或 integer；method 非空且不超过 256 字节；
/// params 为 object；拒绝未知字段。
pub fn jsonrpc_request_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "JSON-RPC Request",
        "description": "JSON-RPC 2.0 请求 schema。Request 必须包含 jsonrpc, id, method；params 必须是 object。",
        "type": "object",
        "required": ["jsonrpc", "id", "method"],
        "additionalProperties": false,
        "properties": {
            "jsonrpc": {
                "type": "string",
                "const": "2.0"
            },
            "id": {
                "oneOf": [
                    { "type": "string" },
                    { "type": "integer" }
                ]
            },
            "method": {
                "type": "string",
                "minLength": 1,
                "maxLength": 256
            },
            "params": {
                "type": "object"
            }
        }
    })
}

/// 生成 JSON-RPC response 的 JSON Schema。
///
/// 约束：response 必须包含 result 或 error 二者之一，不可同时存在或同时缺失；
/// error.code 为整数；拒绝未知字段。
pub fn jsonrpc_response_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "JSON-RPC Response",
        "description": "JSON-RPC 2.0 响应 schema。Response 必须包含 result 或 error 二者之一，不可同时存在或同时缺失。",
        "type": "object",
        "required": ["jsonrpc", "id"],
        "additionalProperties": false,
        "oneOf": [
            { "required": ["result"] },
            { "required": ["error"] }
        ],
        "properties": {
            "jsonrpc": {
                "type": "string",
                "const": "2.0"
            },
            "id": {
                "oneOf": [
                    { "type": "string" },
                    { "type": "integer" },
                    { "type": "null" }
                ]
            },
            "result": {},
            "error": {
                "type": "object",
                "required": ["code", "message"],
                "additionalProperties": false,
                "properties": {
                    "code": { "type": "integer" },
                    "message": { "type": "string" },
                    "data": {}
                }
            }
        }
    })
}

/// 生成 event envelope 的 JSON Schema。
///
/// 约束：事件是 JSON-RPC notification（不带 id 字段）；
/// seq 从 1 开始按 stream 严格递增；拒绝未知字段。
pub fn event_envelope_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Event Envelope",
        "description": "Sagent 事件通知 envelope schema。事件是 JSON-RPC notification（不带 id 字段），seq 从 1 开始按 stream 严格递增。",
        "type": "object",
        "required": ["jsonrpc", "method", "params"],
        "additionalProperties": false,
        "properties": {
            "jsonrpc": {
                "type": "string",
                "const": "2.0"
            },
            "method": {
                "type": "string",
                "minLength": 1
            },
            "params": {
                "type": "object",
                "required": ["event_id", "seq", "timestamp", "data"],
                "additionalProperties": false,
                "properties": {
                    "event_id": { "type": "string" },
                    "session_id": { "type": "string" },
                    "turn_id": { "type": "string" },
                    "seq": { "type": "integer", "minimum": 1 },
                    "timestamp": { "type": "string", "format": "date-time" },
                    "data": { "type": "object" }
                }
            }
        }
    })
}

/// 生成 protocol.describe 响应的 JSON Schema。
///
/// 约束：protocol 必须为 "sagent.rpc"；version 为 >= 1 的整数；
/// features 为字符串数组。
pub fn protocol_describe_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Protocol Describe Response",
        "description": "protocol.describe 方法的响应 schema。返回协议版本、runtime 版本和 capabilities。",
        "type": "object",
        "required": ["protocol", "version", "runtime_version", "features"],
        "additionalProperties": false,
        "properties": {
            "protocol": {
                "type": "string",
                "const": "sagent.rpc"
            },
            "version": {
                "type": "integer",
                "minimum": 1
            },
            "runtime_version": {
                "type": "string"
            },
            "features": {
                "type": "array",
                "items": { "type": "string" }
            }
        }
    })
}

/// 获取所有 schema 的列表，用于批量生成。
///
/// 返回 (文件名, schema JSON) 的迭代器，方便 CLI 命令一次性写出所有 schema 文件。
///
/// # 示例
///
/// ```rust
/// use sagent_api::schema::all_schemas;
/// let schemas = all_schemas();
/// assert_eq!(schemas.len(), 4);
/// ```
pub fn all_schemas() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("jsonrpc-request.schema.json", jsonrpc_request_schema()),
        ("jsonrpc-response.schema.json", jsonrpc_response_schema()),
        ("event-envelope.schema.json", event_envelope_schema()),
        ("protocol-describe.schema.json", protocol_describe_schema()),
    ]
}
