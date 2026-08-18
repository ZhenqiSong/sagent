# sagent 协议定义

本目录包含 sagent 项目的协议定义，包括 JSON Schema、fixture 和协议决策文档。

## 目录结构

```text
protocols/
├── schemas/              # JSON Schema 定义（由 Rust 代码生成）
│   ├── jsonrpc-request.schema.json
│   ├── jsonrpc-response.schema.json
│   ├── event-envelope.schema.json
│   ├── protocol-describe.schema.json
│   └── session-rpc.schema.json
├── fixtures/             # 协议 fixture（valid + invalid）
│   ├── valid/            # 合法 fixture，应通过 schema 校验
│   └── invalid/          # 非法 fixture，应被 schema 拒绝
├── protocol-decisions.md # 协议设计决策记录
├── reference-notes.md    # Python 参考代码分析
└── README.md             # 本文件
```

## Schema 生成

所有 JSON Schema 文件由 Rust 代码生成，不得手工编辑。修改 Rust schema 定义后：

```bash
# 生成 schema 文件
cargo run --bin sagent -- protocol generate-schemas

# 验证无漂移（CI 中执行）
git diff --exit-code -- protocols/schemas
```

Schema 的权威来源是 `crates/sagent-api/src/schema.rs` 中的 Rust 函数。

## Fixture 分类

### Valid fixtures

| 文件 | 描述 |
| --- | --- |
| `rpc-echo-request.json` | rpc.echo 请求 |
| `rpc-echo-response.json` | rpc.echo 响应 |
| `protocol-describe-request.json` | protocol.describe 请求 |
| `protocol-describe-response.json` | protocol.describe 响应 |
| `health-request.json` | health.get 请求 |
| `health-response.json` | health.get 响应 |
| `message-delta-event.json` | message.delta 事件通知 |
| `tool-start-event.json` | tool.start 事件通知 |
| `event-no-session.json` | 无 session_id 的全局事件 |
| `error-protocol-version-unsupported.json` | 协议版本不支持错误 |
| `error-capability-unsupported.json` | 能力不支持错误 |
| `message.json` | Message 类型 fixture |
| `tool-call.json` | ToolCall 类型 fixture |
| `tool-definition.json` | ToolDefinition 类型 fixture |
| `model-event.json` | ModelEvent 类型 fixture |
| `session-*-request.json` | Phase 1 Session RPC 请求 |

### Invalid fixtures

| 文件 | 描述 | 预期错误 |
| --- | --- | --- |
| `missing-jsonrpc.json` | 缺少 jsonrpc 字段 | -32600 |
| `wrong-jsonrpc-version.json` | jsonrpc 不是 "2.0" | -32600 |
| `missing-method.json` | 缺少 method 字段 | -32600 |
| `unknown-envelope-field.json` | 顶层 envelope 含未知字段 | -32600 |
| `null-request-id.json` | request id 为 null | -32600 |
| `both-result-and-error.json` | response 同时含 result 和 error | schema 拒绝 |
| `neither-result-nor-error.json` | response 同时缺少 result 和 error | schema 拒绝 |
| `error-code-not-integer.json` | error code 不是整数 | schema 拒绝 |
| `event-with-id.json` | event 带 id 字段 | schema 拒绝 |
| `seq-zero.json` | seq 为 0 | schema 拒绝 |
| `event-missing-event-id.json` | event 缺少 event_id | schema 拒绝 |
| `seq-negative.json` | seq 为负数 | schema 拒绝 |
| `params-is-array.json` | params 为数组 | -32602 |
| `params-is-string.json` | params 为字符串 | -32602 |
| `unknown-method.json` | 未知方法 | -32601 |
| `unsupported-protocol-version.json` | 不支持的协议版本 | 业务层拒绝 |
| `error-with-sensitive-data.json` | 错误含敏感数据 | 日志脱敏验证 |

## 测试

```bash
# Rust 类型序列化测试
cargo test -p sagent-types --test serialization

# Schema 一致性测试
cargo test -p sagent-api --test schema_tests

# 端到端 stdio conformance 测试
cargo test -p sagent --test stdio_echo
```

## 协议版本

当前协议版本：`sagent.rpc` v1。协议版本与 Runtime 版本独立管理。Phase 1 方法集合新增：`session.create`、`session.list`、`session.get`、`session.resume`、`session.subscribe`。
