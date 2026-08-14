# Sagent 协议 v1

本文档定义 Sagent JSON-RPC 2.0 协议的 v1 版本，包括请求/响应/事件格式、错误码、协议版本协商和能力声明。所有实现必须遵守本文档规定的 wire contract。

## 1. 协议标识

| 字段 | 值 |
| --- | --- |
| 协议族 | `sagent.rpc` |
| 主版本 | `1` |
| 传输格式 | newline-delimited JSON（每行一个完整 JSON-RPC 消息） |
| 字符编码 | UTF-8 |

协议版本与 Runtime 发布版本分离。`version` 字段为整数主协议版本，不兼容变化必须递增。`runtime_version` 仅供展示，不用于协议协商。

## 2. JSON-RPC 2.0 基础

Sagent 基于 JSON-RPC 2.0，并在此基础上增加了以下约束：

### 2.1 版本

`jsonrpc` 字段固定为 `"2.0"`。其他值一律拒绝（返回 `-32600 InvalidRequest`）。

### 2.2 Request ID

- 支持 `string` 和 `number`（整数）两种类型
- 不支持 `null` 作为可关联请求 ID
- 客户端生成的 request id 原样回传
- 推荐使用带前缀的字符串（如 `"req-1"`），避免纯数字 ID 的歧义

### 2.3 params

- `params` 必须是 JSON object
- 没有参数时使用 `{}`
- 不接受数组或字符串类型的 params
- 类型错误返回 `-32602 InvalidParams`

### 2.4 未知字段

协议 envelope 级别拒绝未知字段（`additionalProperties: false`）。业务 metadata 内部可以保留未知字段，但顶层 envelope 不允许多余属性。

### 2.5 单行限制

- 单行最大 1 MiB
- method 最大 256 字节
- `id` 最大 256 字节
- 超限输入返回 `-32003 PayloadTooLarge`

## 3. 消息类型

### 3.1 Request

```json
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "method": "rpc.echo",
  "params": {"value": "hello"}
}
```

必填字段：`jsonrpc`、`id`、`method`。`params` 默认为 `{}`。

### 3.2 Success Response

```json
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "result": {"value": "hello"}
}
```

必填字段：`jsonrpc`、`id`、`result`。

### 3.3 Error Response

```json
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "error": {
    "code": -32602,
    "message": "Invalid params",
    "data": {"field": "value"}
  }
}
```

必填字段：`jsonrpc`、`id`、`error`。`error.code` 为整数，`error.message` 为字符串，`error.data` 可选。无法识别原始 request ID 时，错误响应的 `id` 使用 JSON `null`。

**重要**：Response 必须包含 `result` 或 `error` 二者之一，不可同时存在或同时缺失。

### 3.4 Event (Notification)

```json
{
  "jsonrpc": "2.0",
  "method": "message.delta",
  "params": {
    "event_id": "evt-1",
    "session_id": "sess-1",
    "turn_id": "turn-1",
    "seq": 1,
    "timestamp": "2026-08-07T12:00:00Z",
    "data": {"delta": "hello"}
  }
}
```

事件是 JSON-RPC notification，**不得包含 `id` 字段**。服务端不返回 response。

### 3.5 Event Params 字段

| 字段 | 类型 | 必须 | 约束 |
| --- | --- | --- | --- |
| `event_id` | string | 是 | 当前 event stream 内唯一 |
| `session_id` | string | 条件 | session 事件必须有；全局事件可省略 |
| `turn_id` | string | 条件 | turn 相关事件必须有 |
| `seq` | integer | 是 | 从 1 开始，按 stream 严格递增 |
| `timestamp` | string | 是 | RFC 3339 UTC 格式 |
| `data` | object | 是 | 事件类型对应 payload |

### 3.6 时间格式

- wire 传输使用 RFC 3339 UTC（如 `2026-08-07T12:00:00Z`）
- 持续时间使用整数 milliseconds
- 存储层可使用 Unix epoch milliseconds（不与 wire 格式混淆）

## 4. 错误码

### 4.1 JSON-RPC 标准错误码

| 错误码 | 名称 | 使用场景 |
| ---: | --- | --- |
| `-32700` | `ParseError` | 输入不是合法 JSON |
| `-32600` | `InvalidRequest` | 顶层 JSON 不是合法 JSON-RPC request |
| `-32601` | `MethodNotFound` | method 未注册 |
| `-32602` | `InvalidParams` | params 缺少、类型错误或不满足 schema |
| `-32603` | `InternalError` | 未分类的服务端错误 |

### 4.2 Sagent 扩展错误码

| 错误码 | 名称 | 使用场景 |
| ---: | --- | --- |
| `-32001` | `ProtocolVersionUnsupported` | 客户端要求不支持的协议版本 |
| `-32002` | `CapabilityUnsupported` | 请求依赖未声明的 capability |
| `-32003` | `PayloadTooLarge` | 单行或单个 payload 超过限制 |
| `-32004` | `SequenceViolation` | 事件或请求序列违反协议约束 |
| `-32005` | `Shutdown` | 服务正在有序退出 |

### 4.3 错误响应规则

- 每个错误码只有一个定义来源（`ErrorCode` enum），相同输入错误在不同入口返回相同 code，不依赖错误字符串匹配
- 未知方法返回 `-32601`（`MethodNotFound`），非法 params 返回 `-32602`（`InvalidParams`），两者不可混淆
- 错误 response 保留 request `id`；无法解析 request 或无可识别 ID 时 `id` 设为 `null`
- 错误 `data` 只能放机器可解析的安全诊断信息，不得包含 stack trace、API key、完整环境变量或本地绝对路径

### 4.4 ErrorCode 类型安全映射

Rust 实现中，所有错误码通过 `ErrorCode` enum 统一管理，提供类型安全的双向转换：

- `ErrorCode::to_i32()` → 整数错误码
- `ErrorCode::from_i32(code)` → 从整数解析为 enum（未知码返回 `None`）
- `ErrorCode::default_message()` → 人类可读的默认错误消息
- `ErrorCode::is_standard()` / `is_extension()` → 区分标准码与扩展码
- `ErrorObject::from_code(code)` → 使用默认消息创建错误对象

示例：
```rust
use sagent_api::error::{ErrorCode, ErrorObject};

// 类型安全构造
let err = ErrorObject::from_code(ErrorCode::MethodNotFound);
assert_eq!(err.code, -32601);

// 带自定义消息
let err = ErrorObject::from_code_with_message(
    ErrorCode::InvalidParams,
    "field 'name' is required"
);

// 附加安全诊断数据
let err = ErrorObject::from_code(ErrorCode::ProtocolVersionUnsupported)
    .with_data(serde_json::json!({
        "requested_version": 99,
        "supported_version": 1
    }));
```

## 5. 协议版本协商

### 5.1 protocol.describe 响应

```json
{
  "protocol": "sagent.rpc",
  "version": 1,
  "runtime_version": "0.1.0",
  "features": ["rpc.echo", "protocol.describe", "health.get"]
}
```

### 5.2 版本规则

1. `protocol` 标识协议族，不随 binary patch version 改变
2. `version` 是整数主协议版本；不兼容变化必须递增
3. `runtime_version` 是 Sagent 发布版本，不能用于替代协议版本协商
4. `features` 是 capability 名称列表；客户端只能调用服务端声明的能力
5. 新增可选字段和新 capability 默认属于兼容变化；改变字段含义属于不兼容变化
6. 客户端发送不支持的协议版本时，服务端返回 `-32001`（`ProtocolVersionUnsupported`），且不执行请求
7. 客户端调用未在 `features` 中声明的方法时，服务端返回 `-32002`（`CapabilityUnsupported`）

### 5.3 Capabilities 能力声明

Phase 0 方法列表的权威来源是 `sagent_types::version::PHASE0_METHODS` 常量：

```rust
pub const PHASE0_METHODS: &[&str] = &["rpc.echo", "protocol.describe", "health.get"];
```

`Capabilities` 类型封装方法注册、查询和校验逻辑：

```rust
use sagent_types::version::Capabilities;

let caps = Capabilities::phase0_defaults();

// 查询能力
assert!(caps.supports("rpc.echo"));
assert!(!caps.supports("session.create"));

// 校验方法是否在 capability 列表中
if !caps.validate_method("unknown.method") {
    // 返回 -32601 MethodNotFound
}
```

**重要**：`protocol.describe` 返回的 `features` 列表必须与 `PHASE0_METHODS` 一致。测试 `protocol_version_features_match_phase0_methods` 强制执行此约束。

## 6. Phase 0 方法集合

| 方法 | 类型 | 作用 |
| --- | --- | --- |
| `rpc.echo` | request/response | 验证 stdio request-response 通道，原样返回 params |
| `protocol.describe` | request/response | 返回协议版本和 capabilities |
| `health.get` | request/response | 返回进程和协议健康状态 |

未来方法（如 `session.create`、`prompt.submit`）在后续 Phase 实现，Phase 0 server 不假装支持这些方法。

## 7. 传输层约束

### 7.1 stdio 通道分工

- **stdin**：一行一个 JSON-RPC request 或 notification
- **stdout**：一行一个 JSON-RPC response 或 event notification（纯协议通道）
- **stderr**：日志和诊断（不得出现协议 response）

### 7.2 处理顺序

1. 从 stdin 读取一整行
2. 忽略空行并继续等待
3. 解析 JSON
4. 验证 JSON-RPC envelope（jsonrpc 版本、method 存在性等）
5. 验证协议版本和 method
6. 对 request 返回 response；对 notification 执行 method 但不返回 response
7. 每个 response 序列化为单行 JSON 并立即 flush
8. stdout 写失败或 BrokenPipe 时有序退出
9. stdin EOF 时正常退出（返回码 0）

### 7.3 行为约束

- 一个 request 对应一个 response
- 两个连续 request 输出两行且顺序不乱
- notification 不输出 response
- 协议错误不能让整个进程 panic
- 不使用 `println!` 把日志写到 stdout

## 8. 兼容性

### 8.1 与 Python Hermes Agent 的不兼容

Sagent 是独立 Rust 实现，不兼容以下 Python 组件：

- Python ABI、Python callable 和动态 import
- 旧 SQLite schema（`hermes_state_common.py` 中的表结构）
- Python 插件 ABI（`tools/registry.py` 的 handler 模型）
- Python 全局状态、线程池和 `tui_gateway/server.py` 的 session dict
- `~/.hermes` 路径和 `hermes_constants.py` 的默认目录
- Python 模块名作为 Rust 模块名

### 8.2 协议兼容策略

- 新增可选字段：兼容变化，不影响现有客户端
- 新增 capability：兼容变化
- 新增 method：兼容变化
- 改变字段含义：不兼容变化，需递增主版本
- 移除 method 或 capability：不兼容变化

## 9. Schema 文件

协议 schema 文件位于 `protocols/schemas/`，由 Rust 代码（`sagent-api/src/schema.rs`）生成：

| 文件 | 用途 |
| --- | --- |
| `jsonrpc-request.schema.json` | Request 校验 |
| `jsonrpc-response.schema.json` | Response 校验 |
| `event-envelope.schema.json` | Event 校验 |
| `protocol-describe.schema.json` | protocol.describe 响应校验 |

Schema 文件由单一 Rust 类型来源生成，禁止手工维护漂移副本。修改 Rust 类型后必须重新生成 schema 文件，CI 会检查生成结果无 diff。

生成命令：
```bash
cargo run --bin sagent -- protocol generate-schemas
git diff --exit-code -- protocols/schemas
```

## 10. 日志系统

### 10.1 通道隔离

- **stdout**：纯 JSON-RPC 协议通道，每行合法 JSON
- **stderr**：所有日志输出，使用 tracing 框架

### 10.2 日志级别

默认 `info`，通过 `RUST_LOG` 环境变量覆盖：
```bash
RUST_LOG=debug cargo run --bin sagent -- rpc stdio
```

### 10.3 结构化字段

每条日志包含：`timestamp`、`level`、`target`、`message`。RPC request 处理期间自动携带 `request_id` span 字段。

### 10.4 敏感数据保护

日志自动脱敏以下字段名（大小写不敏感）的值：`token`、`secret`、`password`、`api_key`、`apikey`、`authorization`、`credential`、`private_key`、`access_key`。敏感值替换为 `***REDACTED***`。

详见 `docs/logging.md`。

## 11. 参考

- [JSON-RPC 2.0 规范](https://www.jsonrpc.org/specification)
- `protocols/protocol-decisions.md`：17 项协议决策记录
- `protocols/reference-notes.md`：Python 参考代码阅读记录
- `protocols/schemas/`：JSON Schema 文件
- `protocols/fixtures/`：合法/非法测试 fixture
