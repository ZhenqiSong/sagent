# Sagent Phase 0 实施操作指南

本文档是 `Sagent Rust Architecture` 的独立实施指南，只覆盖 Phase 0：项目基础与协议设计。

Phase 0 的目标不是实现完整 Agent，而是建立一个可以被后续 Runtime、CLI、TUI、Desktop 和
插件共同依赖的工程骨架与协议基线。完成本阶段后，项目应当能够编译、生成并校验协议 schema、
通过 stdio 运行一个最小 JSON-RPC echo server，并在 Linux、macOS、Windows 的 CI 上完成基础编译。

本文档中的路径分为两类：

- `Sagent 路径`：相对于新建的 Sagent Rust 仓库根目录。
- `Python 参考路径`：相对于当前 Hermes Python 仓库根目录，只用于理解现有行为，不作为 Rust
  项目的目录或兼容性约束。

## 1. 范围与最终产出

### 1.1 Phase 0 必须完成

1. 初始化独立的 Cargo Workspace。
2. 建立 `sagent-types` crate。
3. 定义稳定的 Message、ToolCall、ToolDefinition、ModelEvent 和协议 Envelope。
4. 定义 JSON-RPC request、response、error、event schema。
5. 定义协议版本、能力声明和错误码。
6. 建立 stderr 结构化日志，不污染 stdout JSON-RPC 通道。
7. 建立 `~/.sagent` 路径规则和测试覆盖。
8. 建立格式化、lint、依赖审计和跨平台 CI。
9. 提供最小 stdio JSON-RPC echo server。
10. 提供协议 fixture、schema 校验和基本 conformance 测试。

### 1.2 Phase 0 明确不做

以下内容必须留到后续 Phase，不得为了“先跑起来”在 Phase 0 偷渡：

- Provider HTTP 请求、SSE、API key 和模型 fallback。
- Agent Loop、Session Actor、SQLite 和消息持久化。
- Terminal、File、Browser 或任何实际工具执行。
- Tool Registry 的运行时发现和插件加载。
- Context Budget、Compression、Memory、Skills、MCP。
- HTTP、WebSocket、TUI、Desktop、Gateway 和 Scheduler。
- Python 代码调用、Python ABI 兼容或旧 SQLite schema 兼容。
- 以当前 Python 模块名为 Rust 模块名的逐文件翻译。

### 1.3 完成后的目录基线

Phase 0 结束时，Sagent 仓库至少应包含：

```text
sagent/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── deny.toml
├── README.md
├── LICENSE
├── crates/
│   ├── sagent-types/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   └── sagent-api/
│       ├── Cargo.toml
│       └── src/lib.rs
├── bins/
│   └── sagent/
│       ├── Cargo.toml
│       └── src/main.rs
├── protocols/
│   ├── schemas/
│   ├── fixtures/
│   └── README.md
├── tests/
│   ├── conformance/
│   └── fixtures/
├── .github/
│   └── workflows/ci.yml
└── docs/
    └── protocol-v1.md
```

`sagent-api` 在 Phase 0 只承载 JSON-RPC 类型、错误和 stdio 协议边界，不实现 Session 或
Agent 业务。`bins/sagent` 只需要提供 `protocol describe`、`rpc echo` 或等价的最小入口。

## 2. Python 参考代码索引

参考代码的作用是帮助确认已有行为、边界条件和失败模式。阅读时必须记录“为什么参考”，
不得直接复制实现细节。

| 主题 | Python 参考路径 | 重点阅读内容 |
| --- | --- | --- |
| Agent 消息和循环 | `run_agent.py` | 消息角色、工具调用回填、响应结束条件；Phase 0 只提取数据契约 |
| 工具 schema | `model_tools.py` | 工具定义的公开形状、工具调用 dispatch 边界 |
| 工具注册元数据 | `tools/registry.py` | name、schema、handler、availability 的职责分离；不迁移动态发现 |
| 工具集合 | `toolsets.py` | 工具集合与具体工具的关系；Phase 0 只记录未来扩展点 |
| Session 表结构 | `hermes_state_common.py` | sessions/messages 的字段来源；不兼容旧 SQLite schema |
| Session 访问逻辑 | `hermes_state.py` | Session 生命周期和消息读取的语义；不在 Phase 0 实现数据库 |
| JSON-RPC 总入口 | `tui_gateway/entry.py` | stdio 生命周期、EOF、信号和 stdout/stderr 分工 |
| JSON-RPC 分发 | `tui_gateway/server.py` | request、response、event、request_id、seq 和错误响应 |
| Transport 抽象 | `tui_gateway/transport.py` | 输出抽象、并发写保护、BrokenPipe 和 peer disconnect |
| 事件发布 | `tui_gateway/event_publisher.py` | 事件封装、外部传输边界；Phase 0 只实现 stdio |
| 路径规则 | `hermes_constants.py` | 环境覆盖、平台默认目录、测试隔离和 profile 边界 |
| 配置读取 | `hermes_cli/config.py` | 配置与 secret 的职责区别；Phase 0 不实现完整 YAML 配置 |
| 日志 | `hermes_logging.py` | 日志文件位置、session context、stderr 和 secret redaction 思路 |
| 协议测试 | `tests/tui_gateway/test_protocol.py` | JSON-RPC 行为测试、无效输入、错误和事件检查 |
| 日志测试 | `tests/test_hermes_logging.py` | 日志初始化、上下文和文件输出行为 |
| 路径测试 | `tests/test_hermes_constants.py` | 环境覆盖、默认路径和平台行为 |
| 工程依赖 | `pyproject.toml` | 依赖分层、锁定和安全审计理念；Rust 依赖需要单独评估 |
| Python 测试入口 | `scripts/run_tests.sh` | CI parity 和 hermetic test 的思路；Rust 使用 Cargo 命令实现等价约束 |

### 2.1 参考代码阅读记录

在开始编码前建立 `protocols/reference-notes.md`，每个参考文件只记录以下三项：

```text
文件：tui_gateway/server.py
保留的行为：JSON-RPC response 必须带 jsonrpc、id、result 或 error；event 使用 notification。
不迁移的实现：全局 session dict、线程池、Python 动态 dispatch。
```

验收要求：记录至少覆盖上表中的 `run_agent.py`、`tui_gateway/server.py`、
`tui_gateway/transport.py`、`hermes_constants.py`、`hermes_logging.py` 和
`tests/tui_gateway/test_protocol.py`。

## 3. 总体验收门槛

下面的命令均在 Sagent 仓库根目录执行。Phase 0 不能只通过 `cargo check` 就宣布完成。

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny check
cargo audit
```

跨平台基础编译由 CI 执行：

```bash
cargo check --workspace --target x86_64-unknown-linux-gnu
cargo check --workspace --target x86_64-apple-darwin
cargo check --workspace --target x86_64-pc-windows-msvc
```

目标平台可以根据 CI runner 调整，但必须覆盖 Linux、macOS 和 Windows。

## 4. Step 0：冻结边界和建立独立仓库

### 目标

确认 Sagent 是独立 Rust 项目，不会把当前 Python 项目变成 Rust 子目录，也不会复用旧的
Python 包入口、旧 SQLite schema 或旧插件协议。

### 操作

1. 在当前 Python 仓库之外创建新的 Sagent 仓库或独立工作目录。
2. 初始化 Git 仓库和 Rust Workspace。
3. 将本指南复制或链接到 Sagent 项目的开发文档中；不要让 Sagent 编译依赖当前 Python 仓库。
4. 写 `README.md`，明确当前项目仅为行为参考。
5. 在 `docs/non-goals.md` 列出本阶段不做的功能。
6. 在 `protocols/reference-notes.md` 建立 Python 参考阅读记录。

### 产出物

- Sagent 独立 Git 仓库。
- `README.md`。
- `docs/non-goals.md`。
- `protocols/reference-notes.md`。
- 初始 `Cargo.toml`。

### 验收标准

- Sagent 仓库可以在没有 Python 虚拟环境、没有 Hermes 安装的机器上执行 `cargo check`。
- Sagent 的 Cargo manifest 不通过 `path` 依赖、build script 或运行时 import 依赖当前 Python 仓库。
- README 明确写出“不兼容旧 Python 模块结构、旧 SQLite schema、Python 插件 ABI”。
- `git status` 中没有生成的密钥、`.env`、本地数据库或编译产物。

### 失败条件

- 为了复用代码将 Python 仓库加入 Cargo workspace。
- 将 `run_agent.py`、`model_tools.py` 或 `tui_gateway` 作为运行时依赖。
- 在 Phase 0 引入 SQLite、HTTP client 或模型 SDK。

## 5. Step 1：建立 Cargo Workspace 和工具链基线

### 目标

建立所有后续 crate 共享的 Rust edition、MSRV、依赖解析和构建命令。

### 操作

在 Workspace 根目录创建 `Cargo.toml`，至少声明 `resolver = "2"`、统一 edition、MSRV 和
workspace dependencies。Phase 0 推荐只引入 serde、serde_json、thiserror、tracing 和
tracing-subscriber；不要因为未来会用到就提前加入 Tokio、reqwest 或 sqlx。

创建 `rust-toolchain.toml`，固定 toolchain channel，并包含 `rustfmt` 和 `clippy` 组件。
创建 `.rustfmt.toml`，只配置团队明确同意的格式选项，避免过早定制。

创建最小 crate：

```bash
cargo new --lib crates/sagent-types
cargo new --lib crates/sagent-api
cargo new --bin bins/sagent
cargo generate-lockfile
```

如果 `cargo new` 改写了根 manifest，必须整理为单一 Workspace manifest，并检查每个成员的
`Cargo.toml` 是否使用 workspace inheritance。

### 产出物

- `Cargo.toml`。
- `Cargo.lock`。
- `rust-toolchain.toml`。
- `.rustfmt.toml`。
- `crates/sagent-types`、`crates/sagent-api` 和 `bins/sagent`。

### 验收标准

```bash
cargo metadata --no-deps --format-version 1
cargo check --workspace
cargo fmt --all -- --check
```

- `cargo metadata` 返回合法 JSON，成员数量和预期一致。
- 每个 crate 都能独立编译。
- `Cargo.lock` 被纳入版本控制。
- 设置 `RUSTFLAGS="-D warnings"` 后，Phase 0 代码无 warning。
- 删除 `target/` 后重新执行 `cargo check --workspace` 仍然成功。

### 参考 Python 路径

- `pyproject.toml`：参考依赖分层和锁定原则，不复制 Python 依赖。
- `scripts/run_tests.sh`：参考统一入口和 CI parity，不复制脚本逻辑。

## 6. Step 2：建立依赖、安全和 CI 工具链

### 目标

在写业务协议之前先固定质量门槛，防止后续把“能编译”误认为“可合并”。

### 操作

1. 添加 `deny.toml`。
2. 配置 `cargo-deny` 检查 advisories、bans、licenses 和 sources。
3. 添加 `cargo-audit` CI 检查。
4. 添加 `.github/workflows/ci.yml`。
5. CI 至少包含 format、check、test、clippy、deny、audit 六类检查。
6. 配置 Linux、macOS、Windows 的基础 `cargo check --workspace`。
7. 在 `CONTRIBUTING.md` 说明本地验收命令。

建议 CI 顺序：

```text
checkout -> toolchain install -> fmt -> check -> test -> clippy -> deny -> audit
```

### 产出物

- `deny.toml`。
- `.github/workflows/ci.yml`。
- `CONTRIBUTING.md`。
- 依赖审计配置和最小 allowlist。

### 验收标准

- Pull Request 在三种操作系统上至少有一次基础编译检查。
- lint、test、audit 任一步失败都会使 CI 失败。
- `cargo deny check` 不依赖开发者本机的额外配置文件。
- 许可证策略不会默认放行未知来源或未声明许可证的依赖。
- CI 日志不会输出 API key、`SAGENT_HOME` 下的敏感文件内容或完整环境变量。
- 新增一个临时未使用依赖时，deny 或 lint 检查可以发现问题。

### 失败条件

- 用 `continue-on-error` 隐藏 audit、clippy 或 test 失败。
- 通过整个 `target/` 目录缓存来绕过真实编译。
- 为了让 CI 通过而加入过宽的许可证或漏洞忽略规则。

## 7. Step 3：从 Python 行为中提取协议契约

### 目标

在定义 Rust 类型前，先明确哪些行为属于公共协议，哪些只是当前 Python 实现细节。

### 操作

阅读以下 Python 参考路径并在 `protocols/reference-notes.md` 留下记录：

1. `run_agent.py`：识别 `system`、`user`、`assistant`、`tool` 四类消息角色，工具调用的
   `id`、`name`、arguments 和 result 关系。
2. `model_tools.py`：识别对模型暴露的工具定义最小字段，不把 `handler`、Python callable
   或 toolset availability 放进跨进程协议。
3. `tui_gateway/server.py`：识别 JSON-RPC request/response/event 的封装、`id`、
   `request_id`、session/turn 关联和事件序列。
4. `tui_gateway/transport.py`：识别 stdout 是协议通道、日志必须走 stderr、写入需要串行化、
   peer 关闭时要干净退出。
5. `tests/tui_gateway/test_protocol.py`：把已有行为测试转化为 Rust conformance 场景，
   不照搬 Python 测试的模块导入方式。

建立 `protocols/protocol-decisions.md`，逐项决定：

- JSON-RPC request 的 `id` 支持 string、number 还是二者都支持。
- notification 是否允许 `id` 缺失。
- `params` 是否必须是 object。
- unknown fields 是忽略、保留还是拒绝。
- 时间戳使用 RFC 3339、Unix milliseconds 还是仅由存储层负责。
- ID 是否由客户端提供、服务端生成，还是两者都允许。
- event 的 `seq` 是每个 session、每个 transport 还是每个进程递增。
- `arguments` 是结构化 JSON object 还是原始 JSON 字符串。
- content 是纯字符串还是可扩展 content parts。

### 推荐的初始结论

- JSON-RPC request `id` 支持 string 和 number，不支持 null 作为可关联请求 ID。
- notification 不带 `id`，服务端不返回 response。
- `params` 必须是 object；没有参数时统一使用 `{}`。
- 协议 envelope 对未知字段默认拒绝，业务 metadata 对未知字段可以保留。
- wire 时间统一使用 RFC 3339 UTC；持续时间使用整数 milliseconds。
- 服务端生成 `event_id`、`turn_id` 和 `seq`；客户端的 request `id` 原样回传。
- `ToolCall.arguments` 使用 JSON object；无法解析的 provider 原始参数在未来 adapter 层处理。
- `Message.content` 使用可扩展 content parts，但 Phase 0 只实现 text part。
- `seq` 在一个 session event stream 内单调递增；没有 session 的全局事件使用独立 stream。

### 验收标准

- `protocols/protocol-decisions.md` 每个问题都有唯一结论。
- 每个结论都能对应到至少一个 Rust 类型、schema 或 conformance fixture。
- 文档明确列出不兼容旧实现的地方。
- 没有把 Python 全局状态、线程池、动态 import 或旧字段名误写成 Rust 公共协议。

## 8. Step 4：实现 `sagent-types`

### 目标

建立不依赖 Tokio、SQLite、HTTP 或 CLI 的公共数据类型 crate。`sagent-types` 是所有后续
crate 的窄腰，必须保持轻量和稳定。

### 操作

在 `crates/sagent-types/src/` 拆分为：

```text
ids.rs       # SessionId、TurnId、MessageId、ToolCallId
message.rs   # Role、ContentPart、Message
tool.rs      # ToolCall、ToolDefinition
event.rs     # ModelEvent 和事件 payload
envelope.rs  # 通用 envelope
version.rs   # protocol name、version、features
lib.rs       # 公开导出
```

建议公共类型具备以下语义：

```rust
pub struct Message {
    pub message_id: MessageId,
    pub role: Role,
    pub content: Vec<ContentPart>,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<ToolCallId>,
    pub created_at: String,
}

pub enum Role { System, User, Assistant, Tool }

pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    pub arguments: serde_json::Map<String, serde_json::Value>,
}

pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}
```

实际实现可以使用 newtype ID、`chrono` 或 `time`，但必须保持以下原则：

- ID 类型不能混用普通字符串导致 `session_id` 和 `tool_call_id` 互相传递。
- `serde` rename 规则集中定义，不在各个调用点手写字段转换。
- `Message` 不包含 Python callable、数据库连接、日志对象或运行时状态。
- `ToolDefinition.input_schema` 是 JSON Schema 数据，不是 Rust trait object。
- `ModelEvent` 是数据，不负责写 stdout、写数据库或调用模型。
- `Serialize` 和 `Deserialize` 的行为必须由测试固定。

为每个公共类型添加正常序列化、反序列化、缺少必填字段、错误 kind 和 JSON round-trip 测试。

### 参考 Python 路径

- `run_agent.py`：消息角色和工具调用流程。
- `model_tools.py`：工具 schema 的公开边界。
- `tools/registry.py`：工具元数据与执行器分离。
- `hermes_state_common.py`：消息和 session 字段的历史来源，仅用于识别未来可能需要的字段。

### 产出物

- `crates/sagent-types/src/*.rs`。
- `crates/sagent-types/tests/serialization.rs`。
- `protocols/fixtures/message.json`。
- `protocols/fixtures/tool-call.json`。
- `protocols/fixtures/tool-definition.json`。
- `protocols/fixtures/model-event.json`。

### 验收标准

```bash
cargo test -p sagent-types
cargo test -p sagent-types --doc
```

- 所有公共类型都可以稳定 round-trip。
- fixture 使用的字段和 Rust derive 实际输出一致。
- JSON 中不出现未经决定的 tuple enum 或内部 enum 表示。
- 错误输入返回可断言的反序列化错误，不发生 panic。
- `sagent-types` 不依赖 Runtime、数据库、HTTP、文件系统或具体 CLI。

## 9. Step 5：定义 JSON-RPC 2.0 基础 schema

### 目标

把 JSON-RPC request、response、error 和 notification/event 的 wire contract 固定下来，并让
schema 可以被工具校验，而不是只存在于 Markdown 中。

### 操作

在 `crates/sagent-api/src/` 建立：

```text
request.rs    # Request、RequestId、Params
response.rs   # Response、Result、Notification
error.rs      # ErrorObject、标准错误码和 Sagent 错误码
event.rs      # EventEnvelope、事件类型和 seq
schema.rs     # schema 生成或 schema 校验入口
lib.rs        # 公开导出
```

定义四类顶层消息：

```json
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "method": "rpc.echo",
  "params": {"value": "hello"}
}
```

```json
{
  "jsonrpc": "2.0",
  "id": "req-1",
  "result": {"value": "hello"}
}
```

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

```json
{
  "jsonrpc": "2.0",
  "method": "message.delta",
  "params": {
    "event_id": "evt-1",
    "session_id": "sess-1",
    "turn_id": "turn-1",
    "seq": 1,
    "timestamp": "2026-08-07T00:00:00Z",
    "data": {"delta": "hello"}
  }
}
```

请求必须包含 `jsonrpc: "2.0"`、`id`、`method`；response 必须包含 result 或 error 二者之一；
notification/event 不得包含 `id`。不得使用一个结构体通过大量 optional 字段同时掩盖 request
和 response 的不变量。

事件 envelope 至少包含：

| 字段 | 类型 | 必须 | 约束 |
| --- | --- | --- | --- |
| `event_id` | string | 是 | 当前 event stream 内唯一 |
| `session_id` | string | 条件 | session 事件必须有；全局事件可以没有 |
| `turn_id` | string | 条件 | turn 相关事件必须有 |
| `seq` | integer | 是 | 从 1 开始，按 stream 严格递增 |
| `timestamp` | string | 是 | RFC 3339 UTC |
| `data` | object | 是 | 事件类型对应 payload |

### Phase 0 方法集合

Phase 0 只实现以下方法：

| 方法 | 类型 | 作用 |
| --- | --- | --- |
| `rpc.echo` | request/response | 验证 stdio request-response 通道 |
| `protocol.describe` | request/response | 返回 protocol version 和 capabilities |
| `health.get` | request/response | 返回进程和协议健康状态 |

文档中可以列出未来方法，如 `session.create` 和 `prompt.submit`，但 Phase 0 server 不得假装
已经支持这些方法。

### 产出物

- `crates/sagent-api/src/request.rs`。
- `crates/sagent-api/src/response.rs`。
- `crates/sagent-api/src/error.rs`。
- `crates/sagent-api/src/event.rs`。
- `crates/sagent-api/src/schema.rs`。
- `protocols/schemas/jsonrpc-request.schema.json`。
- `protocols/schemas/jsonrpc-response.schema.json`。
- `protocols/schemas/event-envelope.schema.json`。
- `protocols/schemas/protocol-describe.schema.json`。
- `docs/protocol-v1.md`。

### 验收标准

- schema 可以校验所有合法 fixture。
- schema 可以拒绝缺失 `jsonrpc`、同时存在 result/error、event 带 request `id` 等非法 fixture。
- `jsonrpc` 版本固定为 `2.0`。
- 协议版本与 Runtime 版本分离，例如 `sagent.rpc` 的 version 为 `1`。
- schema 文件由代码生成或由单一 Rust 类型来源生成，不能存在手工维护的漂移副本。
- `docs/protocol-v1.md` 对每个必填字段、错误行为和兼容策略有说明。

## 10. Step 6：定义错误码、协议版本和能力声明

### 目标

让客户端可以区分输入错误、方法不存在、协议不兼容和服务内部失败，避免所有失败都变成
无法处理的字符串。

### 操作

在 `crates/sagent-api/src/error.rs` 定义 JSON-RPC 标准错误和 Sagent 扩展错误：

| 错误码 | 名称 | 使用场景 |
| ---: | --- | --- |
| `-32700` | `ParseError` | 一行输入不是合法 JSON |
| `-32600` | `InvalidRequest` | 顶层 JSON 不是合法 JSON-RPC request |
| `-32601` | `MethodNotFound` | method 未注册 |
| `-32602` | `InvalidParams` | params 缺少、类型错误或不满足 schema |
| `-32603` | `InternalError` | 未分类的服务端错误 |
| `-32001` | `ProtocolVersionUnsupported` | 客户端要求不支持的协议版本 |
| `-32002` | `CapabilityUnsupported` | 请求依赖未声明的 capability |
| `-32003` | `PayloadTooLarge` | 单行或单个 payload 超过限制 |
| `-32004` | `SequenceViolation` | 事件或请求序列违反协议约束 |
| `-32005` | `Shutdown` | 服务正在有序退出 |

错误对象必须包含 `code` 和稳定的 `message`，可选 `data` 只能放机器可解析的安全诊断信息。
不得把 stack trace、API key、完整环境变量或本地绝对路径默认放入 response。

定义协议版本对象：

```json
{
  "protocol": "sagent.rpc",
  "version": 1,
  "runtime_version": "0.1.0",
  "features": ["rpc.echo", "protocol.describe", "health.get"]
}
```

版本规则：

1. `protocol` 标识协议族，不随 binary patch version 改变。
2. `version` 是整数主协议版本；不兼容变化必须递增。
3. `runtime_version` 是 Sagent 发布版本，不能用于替代协议版本协商。
4. `features` 是 capability 名称列表；客户端只能调用服务端声明的能力。
5. 新增可选字段和新 capability 默认属于兼容变化；改变字段含义属于不兼容变化。

### 产出物

- `ProtocolVersion` 和 `Capabilities` 类型。
- 错误码 enum 及其 JSON 映射。
- `protocol.describe` 的固定 fixture。
- `docs/protocol-v1.md` 的版本兼容章节。
- 非法版本、未知 capability 和错误 data 的测试 fixture。

### 验收标准

- 每个错误码只有一个定义来源。
- 相同输入错误在不同入口返回相同 code，而不是依赖错误字符串匹配。
- 未知方法返回 `-32601`，非法 params 返回 `-32602`，两者不可混淆。
- `protocol.describe` 返回的 feature 列表与实际注册的方法一致。
- 客户端发送不支持的协议版本时，服务端返回 `-32001`，且不执行请求。
- 错误 response 保留 request `id`；无法解析 request 或没有可识别 ID 时使用 null 或省略，规则在文档中固定。

### 参考 Python 路径

- `tui_gateway/server.py`：参考现有错误 response 和 dispatch 边界。
- `tui_gateway/entry.py`：参考 parse error、EOF 和退出行为。
- `tests/tui_gateway/test_protocol.py`：参考错误输入测试场景。

## 11. Step 7：实现最小 stdio JSON-RPC echo server

### 目标

用真实子进程证明协议可以跨进程工作。该 server 是 Phase 0 的最小可执行闭环，不应包含
Agent、Session 或工具执行逻辑。

### 操作

在 `bins/sagent/src/` 建立：

```text
main.rs       # CLI 入口和退出码
stdio.rs      # stdin line reader、stdout writer、stderr 错误路径
dispatcher.rs # protocol.describe、health.get、rpc.echo
```

建议运行方式：

```bash
cargo run --bin sagent -- rpc stdio
```

输入输出协议采用 newline-delimited JSON：

```text
stdin:  一行一个 JSON-RPC request 或 notification
stdout: 一行一个 JSON-RPC response 或 event notification
stderr: 日志和诊断，不得出现协议 response
```

处理顺序：

1. 从 stdin 读取一整行，不把多行 JSON 当作一个 request。
2. 空行策略必须固定；建议忽略空行并继续等待。
3. 先解析 JSON，再验证 JSON-RPC envelope。
4. 验证协议版本和 method。
5. 对 `rpc.echo` 原样返回 object params，但不能原样回显 request envelope。
6. 对 notification 执行 method 但不发送 response；Phase 0 可拒绝不支持的 notification。
7. 每个 response 序列化为单行 JSON 并立即 flush。
8. stdout 写失败或 BrokenPipe 时走有序退出路径，不输出 traceback 到 stdout。
9. stdin EOF 时正常退出，返回码为 0；协议错误本身不能让整个进程 panic。

建议限制：

- 单行最大 1 MiB。
- method 最大 256 字节。
- `id` 最大 256 字节或等价的明确限制。
- 任何超限输入返回 `-32003`，随后继续处理下一行；如果无法安全继续，可记录 stderr 后退出，规则必须测试固定。

### 产出物

- `bins/sagent/src/main.rs`。
- `bins/sagent/src/stdio.rs`。
- `bins/sagent/src/dispatcher.rs`。
- `tests/conformance/stdio_echo.rs`。
- `protocols/fixtures/rpc-echo-request.json`。
- `protocols/fixtures/rpc-echo-response.json`。

### 验收标准

手工测试：

```bash
printf '%s\n' '{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{"value":"hello"}}' \
  | cargo run --quiet --bin sagent -- rpc stdio
```

必须得到一行合法 response，且包含：

```json
{"jsonrpc":"2.0","id":"1","result":{"value":"hello"}}
```

自动测试必须覆盖：

- 一个 request 对应一个 response。
- 两个连续 request 输出两行且顺序不乱。
- notification 不输出 response。
- 非法 JSON 返回 `-32700` 或文档规定的 parse error。
- 缺少 method 返回 `-32600`。
- 未知 method 返回 `-32601`。
- params 类型错误返回 `-32602`。
- response 只包含 result 或 error 之一。
- stdout 每行都是完整 JSON，stderr 不混入 stdout。
- stdin EOF 正常退出。
- 下游关闭 stdout 时不产生未处理 panic。
- 输入单行超过限制时行为符合文档。

### 参考 Python 路径

- `tui_gateway/entry.py`：stdio 主循环、EOF、退出和信号边界。
- `tui_gateway/transport.py`：stdout lock、flush、BrokenPipe 和 peer gone 行为。
- `tui_gateway/server.py`：request dispatch 和 response envelope。
- `tests/tui_gateway/test_protocol.py`：协议测试场景。

### 失败条件

- 使用 `println!` 把日志写到 stdout。
- 用 panic 处理用户输入错误。
- 将 JSON-RPC echo server 直接连接到真实 Agent 或模型。
- 读取 stdin 到 EOF 后才批量输出 response，导致交互式客户端无法收到即时响应。

## 12. Step 8：建立 `~/.sagent` 路径规则

### 目标

在没有引入完整配置系统的前提下，确定 Sagent 的本地 home、日志、缓存和运行时文件边界，
并保证 Linux、macOS、Windows 行为可预测。

### 操作

在 `crates/sagent-api` 或独立的 `crates/sagent-config` 中实现路径解析。若严格按 Phase 0
最小范围，可以先将路径模块放在 `sagent-types` 之外的 `sagent-api`，但不得把路径逻辑写入
binary 的多个命令分支。

定义以下 API：

```text
SagentHome::discover()
SagentHome::from_env()
SagentHome::config_dir()
SagentHome::logs_dir()
SagentHome::cache_dir()
SagentHome::runtime_dir()
```

推荐规则：

| 平台 | 默认 home |
| --- | --- |
| Linux | `$HOME/.sagent` |
| macOS | `$HOME/.sagent` |
| Windows | `%USERPROFILE%\\.sagent` |

允许 `SAGENT_HOME` 覆盖默认路径。该变量属于内部/部署覆盖，不应在用户文档中扩展成大量
行为开关。所有目录都使用 `PathBuf`，不能拼接硬编码 `/`。

建议子目录：

```text
<SAGENT_HOME>/
├── config/
├── logs/
├── cache/
└── runtime/
```

Phase 0 不创建数据库、sessions 或 secrets 目录。未来添加敏感文件时再明确权限和生命周期。
目录创建应采用显式初始化，不要在纯路径查询函数中产生隐式副作用；如果选择自动创建，必须
在 API 文档中声明并测试。

### 产出物

- 路径解析模块。
- `docs/paths.md`。
- Linux、macOS、Windows 默认路径 fixture。
- `SAGENT_HOME` 覆盖测试。

### 验收标准

- 设置 `SAGENT_HOME=/tmp/test-sagent` 时所有派生路径都位于该目录下。
- 未设置覆盖变量时，路径符合当前平台规则。
- `SAGENT_HOME` 为相对路径、空字符串或包含 NUL 时有明确错误行为。
- 不调用当前 Python 项目的 `hermes_constants.py`，不硬编码 `~/.hermes`。
- 路径测试不写入真实用户 home，使用临时目录或进程级环境隔离。
- Windows 路径测试不依赖 POSIX `/tmp`、`chmod` 或 `/proc`。
- 同一个进程内重复解析得到相同路径，不因当前工作目录变化而漂移。

### 参考 Python 路径

- `hermes_constants.py`：参考环境覆盖、平台默认目录和 profile-safe 原则。
- `tests/test_hermes_constants.py`：参考测试环境隔离和平台分支。
- `hermes_cli/config.py`：参考配置与 secret 文件的目录边界。

## 13. Step 9：建立 tracing 日志并保护 stdout 协议通道

### 目标

让 stdio server 在有日志的情况下仍能保持 stdout 为纯 JSON-RPC，并提供可关联的诊断信息。

### 操作

实现日志初始化模块，建议位置为 `crates/sagent-api/src/logging.rs` 或独立的
`crates/sagent-observability`。Phase 0 只需要 tracing subscriber，不需要指标、遥测导出或
第三方 telemetry。

日志规则：

1. 所有日志写 stderr 或文件，绝不能写 stdout。
2. 默认级别为 `info`，通过 `RUST_LOG` 或明确的 CLI 参数覆盖。
3. 每个 RPC request 生成或携带 `request_id` span 字段。
4. 解析失败、未知方法、退出原因和 BrokenPipe 都必须有结构化日志。
5. 日志中不得打印 secret、完整 request params 或未经裁剪的用户内容。
6. server 启动时记录 protocol version、runtime version 和 enabled capabilities。
7. 日志初始化必须幂等，测试中重复初始化不能添加重复 subscriber 或 panic。

最小字段建议：

```text
timestamp
level
target
message
request_id (optional)
session_id (optional, Phase 0 usually absent)
error_code (optional)
```

不要在 Phase 0 直接复制 `hermes_logging.py` 的多文件 rotating handler、profile context 或
secret redaction 全部实现；只提取“协议通道与日志通道隔离”和“结构化关联字段”这两个契约。

### 产出物

- tracing 初始化模块。
- `docs/logging.md`。
- 日志输出和 stdout 隔离测试。
- 错误路径日志测试。

### 验收标准

- 执行 stdio server 时 stdout 每一行都能被 JSON parser 解析。
- 设置 `RUST_LOG=debug` 只增加 stderr 日志，不改变 stdout response。
- 错误 response 包含稳定 code，stderr 日志包含相同 code 或 request_id。
- request params 中放入类似 token 的测试字符串后，日志不会原样输出该值。
- 重复调用日志初始化不会出现重复行或 panic。
- 日志写入失败不会破坏 RPC response 的 schema；具体降级行为有测试。

### 参考 Python 路径

- `hermes_logging.py`：参考 session context、组件日志和 stderr 容错。
- `tui_gateway/entry.py`：参考 stdout 协议通道和 stderr crash/exit 诊断。
- `tui_gateway/transport.py`：参考写入异常和 peer disconnect 分类。
- `tests/test_hermes_logging.py`：参考日志初始化行为测试。

## 14. Step 10：生成 schema、fixture 和 conformance 测试

### 目标

确保协议不是“Rust 当前实现的偶然 JSON 输出”，而是有独立 fixture 和正反向校验的公共契约。

### 操作

建立以下目录：

```text
protocols/
├── schemas/
│   ├── jsonrpc-request.schema.json
│   ├── jsonrpc-response.schema.json
│   ├── event-envelope.schema.json
│   └── protocol-describe.schema.json
├── fixtures/
│   ├── valid/
│   └── invalid/
└── README.md
```

合法 fixture 至少包括：

- `rpc-echo-request.json`。
- `rpc-echo-response.json`。
- `protocol-describe-response.json`。
- `health-response.json`。
- `message-delta-event.json`。
- `tool-call-event.json`。
- string request ID 和 numeric request ID 各一个。

非法 fixture 至少包括：

- 缺少 `jsonrpc`。
- `jsonrpc` 不是 `2.0`。
- 缺少 method。
- response 同时含 result 和 error。
- response 同时缺少 result 和 error。
- event 带 `id`。
- `seq` 为负数、零或非整数。
- event 缺少必需 envelope 字段。
- error code 不是整数。
- params 为数组或字符串。
- 超出长度限制的 method 或 request line。

测试分三层：

1. **Rust 类型测试**：serde 序列化和反序列化。
2. **schema 测试**：fixture 对 schema 的正向和反向校验。
3. **进程测试**：启动 `sagent rpc stdio`，写入多行 request，读取并校验真实 stdout。

如果 schema 生成依赖额外工具，必须提供可重复的生成命令，并在 CI 检查生成结果无 diff：

```bash
cargo run --bin sagent -- protocol generate-schemas
git diff --exit-code -- protocols/schemas
```

Phase 0 可以使用临时 schema 校验器，但最终选型必须固定在 `CONTRIBUTING.md`，不能要求开发者
手工打开 JSON 检查。

### 产出物

- 完整 valid/invalid fixtures。
- schema 校验测试。
- 端到端 stdio conformance 测试。
- schema 生成命令或生成校验脚本。

### 验收标准

- 所有 valid fixture 通过 schema 校验。
- 所有 invalid fixture 被拒绝，且失败原因可定位到字段或约束。
- 运行时 serializer 输出可以被 schema 接受。
- schema 允许的 request 至少能被 runtime 处理，不能出现 schema 接受但 runtime 一律拒绝的空协议。
- conformance 测试在 Linux、macOS、Windows 至少有一套执行记录；平台特定差异必须显式标记。
- 修改 Rust 字段后，如果未重新生成 schema，CI 会失败。

### 参考 Python 路径

- `tests/tui_gateway/test_protocol.py`：参考行为测试转化方向。
- `tui_gateway/server.py`：参考 event 和 response 的现有 envelope。
- `tui_gateway/transport.py`：参考真实 stdio 写出边界。

## 15. Step 11：完成文档、代码审查和 Phase 0 总验收

### 目标

确认 Phase 0 的产出足以支撑 Phase 1，而不是留下隐含的协议分歧、跨平台问题或不受控依赖。

### 操作

依次执行：

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny check
cargo audit
cargo run --quiet --bin sagent -- protocol describe
```

执行 stdio 端到端检查：

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":"1","method":"protocol.describe","params":{}}' \
  '{"jsonrpc":"2.0","id":"2","method":"rpc.echo","params":{"value":"ok"}}' \
  '{"jsonrpc":"2.0","id":"3","method":"health.get","params":{}}' \
  | cargo run --quiet --bin sagent -- rpc stdio
```

逐项审查：

1. `sagent-types` 是否仍然是轻量窄腰。
2. 协议文档、Rust 类型、schema 和 fixture 是否一致。
3. 所有 stdout 输出是否都来自协议 writer。
4. 所有错误是否有稳定 code 和测试。
5. 是否存在无上界依赖、未审计 git dependency 或过宽 license allowlist。
6. 是否意外实现了 Phase 1 以后功能。
7. 是否所有文档中的 Python 路径都是相对路径。
8. 是否将 `~/.hermes`、Python 模块名或旧表字段误写成 Sagent 运行时要求。

### 最终验收标准

Phase 0 只有在以下条件全部满足时才算完成：

- `cargo check --workspace --all-targets` 通过。
- `cargo test --workspace` 通过，包含 schema、fixture 和真实 stdio 子进程测试。
- `cargo fmt --all -- --check` 通过。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过。
- `cargo deny check` 和 `cargo audit` 通过或有已审查、最小范围的例外记录。
- Linux、macOS、Windows CI 基础编译通过。
- `protocol.describe` 能返回协议版本、runtime 版本和实际 capabilities。
- `rpc.echo` 能在真实 stdio 子进程中完成 request-response。
- 非法 JSON、非法 request、未知 method、非法 params 都有稳定错误码。
- stdout 只包含 JSON-RPC frame，日志只走 stderr 或日志文件。
- `SAGENT_HOME` 覆盖和三平台默认路径都有测试。
- 参考代码记录至少覆盖 Agent、工具、JSON-RPC、Transport、路径、日志和协议测试入口。
- 没有引入 Provider、Session、SQLite、真实 Tool 或任何模型调用。

### Phase 0 交付检查表

```text
[ ] 独立 Sagent 仓库和 README
[ ] docs/non-goals.md
[ ] protocols/reference-notes.md
[ ] protocols/protocol-decisions.md
[ ] Cargo.toml 和 Cargo.lock
[ ] rust-toolchain.toml 和 .rustfmt.toml
[ ] deny.toml 和 cargo-audit CI
[ ] Linux/macOS/Windows CI 基础编译
[ ] sagent-types 公共类型
[ ] sagent-api JSON-RPC 类型和错误码
[ ] protocol v1 文档
[ ] JSON schema 和 valid/invalid fixtures
[ ] stdio echo server
[ ] protocol.describe 和 health.get
[ ] tracing 日志和 stdout 隔离
[ ] SAGENT_HOME 路径模块
[ ] Rust 单元、schema、conformance、进程测试
[ ] 最终 cargo fmt/check/test/clippy/deny/audit 全部通过
```

## 16. Phase 1 开始前的交接条件

Phase 0 完成后，下一阶段只能在以下边界上继续：

- `sagent-types` 的 Message、ToolCall、ToolDefinition 和 Event 类型被视为公共 API。
- JSON-RPC protocol version、错误码和 event envelope 已冻结；新增内容走兼容性评审。
- stdio transport 可以被未来 Session Actor 或 API 层复用，但 Phase 0 的 dispatcher 不应被当作
  Agent runtime。
- `SagentHome` 是未来配置、session 和日志路径的唯一入口。
- tracing 的 request/session correlation 字段可被未来 Runtime 继承。
- Phase 1 可以新增 `sagent-config`、`sagent-session` 和 SQLite，但不能绕过本阶段的类型和
  协议边界重新定义消息结构。

如果以上交接条件无法满足，应先修复 Phase 0，不要在 Phase 1 用兼容层掩盖协议未决问题。
