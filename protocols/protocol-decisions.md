# Sagent 协议决策记录

本文档逐项决定 JSON-RPC 协议和消息契约中所有模糊点。每个问题只有一个最终结论，
每个结论都对应到至少一个 Rust 类型、JSON schema 或 conformance fixture。

本文档是 Sagent 项目的协议权威来源（SSOT），代码实现、schema 文件和测试必须与此一致。

---

## 1. JSON-RPC Request ID 类型

**问题**：JSON-RPC request 的 `id` 支持 string、number 还是二者都支持？

**结论**：**同时支持 string 和 number，不支持 null。**

**理由**：JSON-RPC 2.0 规范允许 string、number 和 null。string 更灵活（支持前缀 + 随机数），number 更紧凑。null 作为可关联 request ID 无意义，不支持。

**对应 Rust 类型**：`sagent_types::ids::RequestId`

```rust
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Number(i64),
}
```

**与 Python 差异**：Python 实现通过 `_normalize_request` 隐式接受任意 JSON 标量。Sagent 明确拒绝 null ID。

---

## 2. Notification 的 id 字段

**问题**：notification 是否允许 `id` 缺失？

**结论**：**Notification 不带 `id` 字段，服务端不返回 response。**

**理由**：JSON-RPC 2.0 规范规定 notification 是没有 `id` 的 request。如果带 `id` 则变为普通 request，服务端必须返回 response。

**对应 Rust 类型**：`sagent_api::request::Notification`（无 `id` 字段）、`sagent_api::event::EventEnvelope`（无 `id` 字段）。

**Schema 约束**：`protocols/schemas/jsonrpc-request.schema.json` 中 notification 的 schema 禁止 `id` 字段。

---

## 3. params 类型约束

**问题**：`params` 是否必须是 object？

**结论**：**`params` 必须是 JSON object。没有参数时统一使用 `{}`。**

**理由**：数组参数语义不清（位置依赖），object 参数通过 key 明确语义。JSON-RPC 2.0 规范推荐使用 object。统一 `{}` 简化空参数处理。

**对应 Rust 类型**：`sagent_api::request::Request.params: serde_json::Value`（反序列化时接受 object；验证逻辑在 dispatcher 中拒绝数组和字符串）。

**Schema 约束**：`protocols/schemas/jsonrpc-request.schema.json` 中 `params` 的 `type` 为 `object`。

**与 Python 差异**：Python 实现中 `_normalize_request` 允许 `params` 为 `null` 并转为 `{}`。Sagent 拒绝 `null`，明确要求 `{}`。

---

## 4. 未知字段处理策略

**问题**：unknown fields 是忽略、保留还是拒绝？

**结论**：

- **协议 envelope 级别**：默认拒绝（`#[serde(deny_unknown_fields)]`）。非法字段意味着协议不兼容，应尽早发现。
- **业务 metadata 级别**：可以保留（通过 `serde_json::Value` 或 `#[serde(flatten)]`）。metadata 允许向前兼容。

**理由**：协议 envelope 是跨进程契约，严格性防止无声数据损坏。业务 metadata（如 message 的 `metadata_json`）是扩展点，允许携带未定义但安全的附加信息。

**对应 Rust 类型**：`sagent_api::request::Request` 使用 `#[serde(deny_unknown_fields)]`。`sagent_types::message::Message` 不拒绝未知字段（通过 `serde_json::Value` 保留）。

**与 Python 差异**：Python `json.loads` 默认接受任意字段。Sagent 在协议层更严格。

---

## 5. 时间戳格式

**问题**：时间戳使用 RFC 3339、Unix milliseconds 还是仅由存储层负责？

**结论**：

- **wire 传输**：RFC 3339 UTC 字符串（如 `"2026-08-07T12:00:00Z"`）。
- **持续时间**：整数 milliseconds（如 `{"timeout_ms": 30000}`）。
- **存储层**：Unix epoch milliseconds（SQLite INTEGER），在读写时转换。

**理由**：RFC 3339 人类可读、跨语言无歧义、JSON 原生支持。存储层使用整数便于排序和范围查询。持续时间使用整数避免解析开销。

**对应 Rust 类型**：

- `sagent_types::message::Message.created_at: String`（RFC 3339）
- `sagent_api::event::EventParams.timestamp: String`（RFC 3339）
- 持续时间在 future 类型中定义为 `u64`（milliseconds）

**Schema 约束**：所有 timestamp 字段的 JSON schema 使用 `"format": "date-time"`。

---

## 6. ID 生成策略

**问题**：ID 是否由客户端提供、服务端生成，还是两者都允许？

**结论**：

- **request `id`**：由客户端生成，服务端原样回传。如果客户端未提供（notification），不生成。
- **`session_id`**：由服务端在 `session.create` 时生成，返回给客户端。
- **`turn_id`**：由服务端在每次 Turn 开始时生成。
- **`event_id`**：由服务端生成，在 event stream 内唯一。
- **`message_id`**：由服务端在消息持久化时生成。
- **`tool_call_id`**：由模型 Provider 生成（作为 tool call 的 `id`），Sagent 透传。

**理由**：request id 由客户端控制便于请求关联。服务端生成的 ID 保证全局唯一性和格式一致性。

**对应 Rust 类型**：`sagent_types::ids` 模块中所有 ID 类型（`SessionId`、`TurnId`、`MessageId`、`ToolCallId`、`EventId`、`RequestId`）。

---

## 7. Event seq 作用域

**问题**：event 的 `seq` 是每个 session、每个 transport 还是每个进程递增？

**结论**：**`seq` 在一个 session event stream 内单调递增，从 1 开始。没有 session 的全局事件使用独立 stream，seq 从 1 开始。**

**理由**：session 级 seq 允许客户端检测单个 session 内的丢事件。全局事件（如 health 状态变化）用独立 stream 避免 seq 跳跃。跨 session 不共享 seq 简化并发模型。

**对应 Rust 类型**：`sagent_api::event::EventParams.seq: u64`。

**Schema 约束**：`protocols/schemas/event-envelope.schema.json` 中 `seq` 为 `type: "integer"`，`minimum: 1`。

**与 Python 差异**：Python 实现中事件通过 `event_publisher` 发布，seq 为全局递增。Sagent 改为 per-session 递增，减少全局状态。

---

## 8. ToolCall arguments 格式

**问题**：`arguments` 是结构化 JSON object 还是原始 JSON 字符串？

**结论**：**`ToolCall.arguments` 使用结构化 JSON object（`serde_json::Map<String, Value>`）。无法解析的 provider 原始参数在未来 adapter 层处理，不进入核心类型。**

**理由**：结构化 object 便于工具执行时的参数提取和校验，避免二次 JSON 解析。adapter 层负责将 provider 的字符串参数转为 object。

**对应 Rust 类型**：`sagent_types::tool::ToolCall.arguments: serde_json::Map<String, serde_json::Value>`。

**与 Python 差异**：Python SDK 中 `tc.function.arguments` 是 JSON 字符串（OpenAI API 格式），持久化时保持字符串。Sagent 在核心类型中使用已解析的 object。

---

## 9. Message content 格式

**问题**：content 是纯字符串还是可扩展 content parts？

**结论**：**使用可扩展 content parts 模型，但 Phase 0 只实现 text part。**

**理由**：content parts 模型支持未来多模态（图片、音频、文件引用等），同时 Phase 0 保持简单。text part 与纯字符串语义等价。

**对应 Rust 类型**：`sagent_types::message::ContentPart`

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
}
```

**Schema 约束**：content parts 使用 `tag` 枚举序列化，Phase 0 的合法类型仅 `"text"`。

**与 Python 差异**：Python 实现使用 OpenAI SDK 的 content 格式（可以是 string 或 content parts 数组）。Sagent 统一为 content parts 数组。

---

## 10. 消息角色集合

**问题**：支持哪些消息角色？

**结论**：**支持四种角色：`system`、`user`、`assistant`、`tool`。不支持 Python 中的 `function`（已废弃）和 `developer`（非标准）。**

**理由**：四种角色是 OpenAI Chat Completions API 的事实标准。`function` 是旧版 function calling 格式，已被 tool calling 取代。`developer` 是少数模型的特殊角色，不在核心协议中。

**对应 Rust 类型**：`sagent_types::message::Role`

```rust
#[serde(rename_all = "lowercase")]
pub enum Role { System, User, Assistant, Tool }
```

**与 Python 差异**：Python 的 `_VALID_API_ROLES` 包含 `function` 和 `developer`。Sagent 只保留四种标准角色。

---

## 11. 消息交替不变量

**问题**：消息序列是否强制角色交替？

**结论**：**强制角色交替：不允许连续两条同角色消息。**

**理由**：模型 API 要求消息列表角色交替。违反此规则会导致 Provider API 返回 400 错误。在 Session 层面做验证，不在类型定义层。

**对应验证逻辑**：`sagent_runtime` 的 Session Actor 在追加消息前检查最后一条消息的角色。

**与 Python 差异**：Python 通过消息列表追加隐式保证（不显式检查）。Sagent 显式验证并返回 `SequenceViolation` 错误码。

---

## 12. Tool Call 配对规则

**问题**：Tool call 和 tool result 如何配对？

**结论**：**Assistant 消息可包含多个 `tool_calls`。每个 `tool_call` 必须有一条对应的 `role: "tool"` 消息，通过 `tool_call_id` 关联。tool call 和其 result 构成不可分割的 pair，压缩时不能拆散。**

**理由**：OpenAI API 要求每个 tool call 都有对应的 tool result 消息。拆散会导致 API 调用失败。

**对应 Rust 类型**：`sagent_types::message::Message.tool_call_id: Option<ToolCallId>`（tool 消息通过此字段关联 assistant 的 tool call）。

---

## 13. Tool Definition 最小字段

**问题**：暴露给模型的工具定义需要哪些最小字段？

**结论**：**`name`（工具名）、`description`（功能描述）、`input_schema`（JSON Schema 参数定义）。`handler`、`check_fn`、`toolset` 归属等运行时元数据不属于跨进程协议。**

**理由**：模型只需要 name + description + parameters schema 来生成 tool call。handler 和 check_fn 是执行层关注点，不应暴露到协议。

**对应 Rust 类型**：`sagent_types::tool::ToolDefinition`

```rust
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}
```

**与 Python 差异**：Python 的 `ToolEntry` 包含 `handler`、`check_fn`、`toolset`、`is_async`、`requires_env` 等 10+ 字段。Sagent 的 `ToolDefinition` 只保留三个字段。

---

## 14. 完成原因（Finish Reason）

**问题**：Turn 完成有哪些合法原因？

**结论**：

| 原因 | 含义 |
|------|------|
| `stop` | 模型正常完成（无 tool call） |
| `tool_calls` | 模型请求工具调用 |
| `length` | 达到最大 token 限制 |
| `content_filter` | 内容被过滤 |
| `interrupted` | 用户手动中断 |
| `budget_exhausted` | 迭代预算耗尽 |
| `error` | 发生不可恢复错误 |

**理由**：区分不同完成原因允许客户端做差异化处理（如 `length` 触发自动 continuation，`interrupted` 显示中断提示）。

**对应 Rust 类型**：`sagent_types::event::ModelEvent::TurnCompleted { reason: String }`（Phase 0 使用 String；Phase 1 可改为 enum）。

---

## 15. 错误响应规范

**问题**：错误响应必须满足什么约束？

**结论**：

1. 错误对象包含 `code`（整数）和 `message`（字符串）。
2. 可选 `data` 只能放机器可解析的安全诊断信息。
3. 不得在 response 中返回 stack trace、API key、完整环境变量或本地绝对路径。
4. 错误 response 保留 request `id`；无法解析 request ID 时返回 `null`。

**对应 Rust 类型**：`sagent_api::error::ErrorObject`、`sagent_api::error::codes`。

**Schema 约束**：`protocols/schemas/jsonrpc-response.schema.json` 的 error schema。

---

## 16. 协议版本兼容规则

**问题**：协议版本如何兼容？

**结论**：

1. `protocol` 字段标识协议族（`"sagent.rpc"`），不随 patch 版本改变。
2. `version` 是整数主协议版本；不兼容变化必须递增。
3. `runtime_version` 是 Sagent 发布版本（如 `"0.1.0"`），仅供展示，不用于协议协商。
4. `features` 是 capability 名称列表；客户端只能调用服务端声明的能力。
5. 新增可选字段和新 capability 是兼容变化；改变字段含义是不兼容变化。

**对应 Rust 类型**：`sagent_types::version::ProtocolVersion`。

---

## 17. 传输层约束

**问题**：stdio transport 有哪些硬性约束？

**结论**：

1. **stdin**：逐行读取，每行一个完整 JSON-RPC 消息（newline-delimited JSON）。
2. **stdout**：逐行写入 JSON-RPC response 或 event notification，立即 flush。
3. **stderr**：仅用于日志和诊断，不出现协议 response。
4. **空行**：忽略并继续等待。
5. **单行上限**：1 MiB。
6. **method 上限**：256 字节。
7. **id 上限**：256 字节。
8. **stdin EOF**：正常退出，返回码 0。
9. **BrokenPipe**：干净退出，不 panic。
10. **写入串行化**：stdout 写入通过 Mutex 保护。

**对应 Rust 类型**：`bins/sagent/src/stdio.rs`（待实现）、`sagent_api::error::codes::PAYLOAD_TOO_LARGE`。

**与 Python 差异**：Python 使用 `threading.Lock` + `_real_stdout` 全局变量。Sagent 使用 `Mutex<BufWriter<Stdout>>`。

---

## 与 Python 实现的不兼容清单

以下行为在 Sagent 中与 Python 实现不同，是主动设计决策而非遗漏：

| 项目 | Python (hermes-agent) | Sagent | 理由 |
|------|-----------------------|--------|------|
| 消息角色 | 支持 `function`、`developer` | 仅 `system/user/assistant/tool` | 精简为标准角色 |
| ToolCall arguments | JSON 字符串 | 结构化 JSON object | 避免二次解析 |
| Content 格式 | string 或 array | 统一 content parts 数组 | 向前兼容多模态 |
| Event seq 作用域 | 全局递增 | per-session 递增 | 减少全局状态 |
| params 为 null | 转为 `{}` | 拒绝，要求 `{}` | 严格性 |
| 未知字段 | 默认接受 | 协议层默认拒绝 | 尽早发现不兼容 |
| 存储路径 | `~/.hermes` | `~/.sagent` | 独立项目 |
| 环境变量 | `HERMES_HOME` | `SAGENT_HOME` | 避免命名冲突 |
| 工具注册 | AST 扫描 + importlib | 显式 `register()` | 无副作用 |
| ToolEntry 字段 | 10+ 字段 | 3 字段（name/description/schema） | 协议与执行分离 |
