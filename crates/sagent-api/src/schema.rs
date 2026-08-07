//! Schema 生成和校验模块。
//!
//! Phase 0 提供基础的 schema 生成入口。
//!
//! @author   songzq
//! @created  2025-08-07
//! @change   2025-08-07 初始版本：Phase 0 Step 0 JSON Schema 生成

/// 生成 JSON-RPC request 的 JSON Schema。
pub fn jsonrpc_request_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "JSON-RPC Request",
        "type": "object",
        "required": ["jsonrpc", "id", "method"],
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
                "minLength": 1
            },
            "params": {
                "type": "object"
            }
        }
    })
}

/// 生成 JSON-RPC response 的 JSON Schema。
pub fn jsonrpc_response_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "JSON-RPC Response",
        "type": "object",
        "required": ["jsonrpc", "id"],
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
                    { "type": "integer" }
                ]
            },
            "result": {},
            "error": {
                "type": "object",
                "required": ["code", "message"],
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
pub fn event_envelope_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Event Envelope",
        "type": "object",
        "required": ["jsonrpc", "method", "params"],
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
