# Sagent Rust Architecture

本文档定义 `Sagent` 的整体技术方案。Sagent 是一个全新的 Rust Agent Runtime，
不兼容当前项目的 Python 模块结构、旧 SQLite 表结构、Python 插件 ABI 或旧版
CLI 内部实现。

当前项目只作为能力和风险的参考来源。Sagent 应该重新设计边界、数据模型、并发
模型和扩展协议，而不是把现有代码逐文件翻译成 Rust。

## 1. 产品定位

Sagent 是一个模块化的本地优先 AI Agent Runtime，提供：

- 多模型 Provider 接入
- 流式对话和工具调用
- 会话持久化与恢复
- 本地终端和文件操作
- 工具权限、审批和取消
- 子代理委托
- Skills、MCP、外部插件
- CLI、TUI、Web、Desktop 和消息 Gateway 接入

核心设计原则：

1. **协议优先**：Runtime、CLI、TUI、Desktop 和插件通过稳定协议交互。
2. **核心窄腰**：Agent Loop、Session、Tool、Provider、Event 是核心，具体能力放在边缘。
3. **本地优先**：默认单机运行，数据放在用户自己的 Sagent Home 中。
4. **Prompt Cache 安全**：一个会话内的系统提示、工具集合和缓存前缀保持稳定。
5. **取消优先**：模型请求、工具执行、子代理和后台任务都必须可取消。
6. **安全默认**：文件、终端、插件和外部服务默认受限。
7. **模块化单体优先**：内部模块解耦，但初期不拆成微服务。

## 2. 设计边界

### 2.1 第一版目标

第一版只需要完成一个可靠的 Agent 闭环：

```text
用户输入
  -> Sagent Runtime
  -> Model Provider
  -> Tool Call
  -> Tool Executor
  -> SQLite Session
  -> 流式输出
```

P0 能力：

| 能力 | 说明 |
| --- | --- |
| Agent Loop | 模型请求、工具调用、结果回填、结束条件 |
| OpenAI 兼容 Provider | OpenAI、OpenRouter、Ollama、LM Studio 等 |
| 流式输出 | 文本增量、推理增量、工具事件 |
| Session | SQLite、WAL、恢复、基础搜索 |
| Terminal | 命令执行、超时、取消、输出限制 |
| File | 读写、补丁、路径安全 |
| Tool Registry | 工具 schema、注册、执行、可用性 |
| CLI | 单次调用和交互式 REPL |
| JSON-RPC | 给 TUI、Desktop 和外部客户端使用 |
| 日志与错误 | 结构化日志、错误分类、诊断 ID |

### 2.2 后续目标

P1：

- 审批系统
- Context Budget
- Context Compression
- Provider fallback 和重试
- 子代理
- 会话搜索和分支
- 基础 Skills

P2：

- 外部进程插件
- MCP Client
- Memory Provider
- Scheduler
- Webhook Gateway
- HTTP/WebSocket API

P3：

- Telegram、Discord 等消息平台
- Browser Automation
- Desktop App
- 多环境 Terminal Backend
- 远程服务和多租户能力

### 2.3 非目标

第一版不做：

- 自研模型推理引擎
- 自研向量数据库
- 一次性支持二十多个消息平台
- Rust 动态库插件 ABI
- 一开始支持 WASM 插件
- 一开始支持 Docker、SSH、Modal、Daytona 等所有执行环境
- 一开始实现完整 Desktop 产品
- 一开始拆分微服务

## 3. 总体架构

```text
                    +------------------------+
                    |       CLI / TUI         |
                    +------------+-----------+
                                 |
                    +------------v-----------+
                    |      Sagent API/RPC      |
                    | Stdio / HTTP / WebSocket |
                    +------------+-----------+
                                 |
                    +------------v-----------+
                    |     Sagent Runtime       |
                    | Agent Supervisor         |
                    | Session Actor            |
                    | Turn Executor            |
                    | Event Bus                |
                    +-------+----------+-------+
                            |          |
                +-----------v--+   +---v-------------+
                | Model Layer   |   | Tool Layer       |
                | Provider      |   | Registry         |
                | Streaming     |   | Executor         |
                | Retry/Fallback|   | Approval         |
                +-------+-------+   +--------+----------+
                        |                    |
                +-------v-------+   +--------v----------+
                | LLM APIs      |   | Local / External  |
                | OpenAI         |   | Terminal          |
                | Anthropic      |   | File              |
                | Gemini         |   | MCP               |
                +---------------+   | Plugin Process    |
                                    +-------------------+

                    +------------------------+
                    | Session Repository      |
                    | SQLite + FTS5 + WAL     |
                    +------------------------+
```

推荐采用“模块化单体 + Actor/Channel 并发模型 + 协议化扩展”。初期所有模块可以
运行在一个进程中，未来再根据实际负载拆分服务。

不建议一开始拆成：

```text
agent-service / tool-service / provider-service / session-service / gateway-service
```

这会提前引入网络序列化、服务发现、分布式一致性和多进程调试成本。

## 4. Rust Workspace

建议目录：

```text
sagent/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── deny.toml
├── README.md
├── LICENSE
│
├── crates/
│   ├── sagent-types/
│   ├── sagent-core/
│   ├── sagent-runtime/
│   ├── sagent-model/
│   ├── sagent-tools/
│   ├── sagent-session/
│   ├── sagent-config/
│   ├── sagent-security/
│   ├── sagent-plugin/
│   ├── sagent-mcp/
│   ├── sagent-api/
│   ├── sagent-gateway/
│   ├── sagent-scheduler/
│   ├── sagent-terminal/
│   ├── sagent-memory/
│   └── sagent-observability/
│
├── bins/
│   ├── sagent/
│   ├── sagentd/
│   └── sagent-plugin-host/
│
├── adapters/
│   ├── provider-openai/
│   ├── provider-anthropic/
│   ├── provider-gemini/
│   ├── channel-webhook/
│   └── browser/
│
├── protocols/
│   ├── jsonrpc/
│   ├── schemas/
│   └── conformance/
│
├── migrations/
│   └── sqlite/
│
├── skills/
│   └── coding/
│
└── tests/
    ├── integration/
    ├── conformance/
    ├── fixtures/
    └── golden/
```

### 4.1 依赖方向

```text
sagent-types
    ^
sagent-core
    ^
sagent-session / sagent-config / sagent-security / sagent-model / sagent-tools
    ^
sagent-runtime
    ^
sagent-api / sagent-gateway / sagent-scheduler
    ^
sagent-cli / sagentd
```

约束：

- `sagent-types` 不依赖 Runtime、数据库或 HTTP。
- `sagent-core` 不依赖 SQLite、CLI、Gateway。
- `sagent-model` 不依赖 Gateway。
- `sagent-tools` 不依赖具体 CLI。
- `sagent-api` 只负责协议和请求分发。
- `sagent-runtime` 负责业务编排。
- Binary crate 负责启动、信号处理和依赖装配。
- 底层 crate 不得反向依赖 binary crate。

## 5. 核心 Crate 设计

### 5.1 `sagent-types`

所有模块共享的数据模型。

```rust
pub struct SessionId(pub String);
pub struct TurnId(pub String);
pub struct ToolCallId(pub String);
pub struct ProviderId(pub String);

pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

pub struct Message {
    pub id: Option<String>,
    pub role: Role,
    pub content: MessageContent,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<ToolCallId>,
    pub metadata: serde_json::Value,
}

pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

pub enum ContentPart {
    Text { text: String },
    ImageUrl { url: String },
    ImageData { media_type: String, data: Vec<u8> },
}

pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    pub arguments: serde_json::Value,
}

pub struct ToolResult {
    pub call_id: ToolCallId,
    pub content: String,
    pub is_error: bool,
}
```

模型事件：

```rust
pub enum ModelEvent {
    TextDelta(String),
    ReasoningDelta(String),
    ToolCallStarted(ToolCall),
    ToolCallDelta { call_id: ToolCallId, arguments_delta: String },
    Usage(Usage),
    Completed(FinishReason),
}
```

### 5.2 `sagent-core`

定义不依赖具体实现的核心接口：

```rust
#[async_trait::async_trait]
pub trait ModelProvider: Send + Sync {
    fn id(&self) -> &ProviderId;

    async fn complete(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelResponse, ModelError>;

    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError>;
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;

    async fn execute(
        &self,
        ctx: ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolOutput, ToolError>;
}

#[async_trait::async_trait]
pub trait MemoryProvider: Send + Sync {
    fn id(&self) -> &str;
    async fn recall(&self, query: &str) -> Result<Vec<MemoryItem>, MemoryError>;
    async fn remember(&self, input: RememberInput) -> Result<(), MemoryError>;
}
```

接口设计要避免把具体实现泄漏到核心，例如：

- 不在 `ModelProvider` 中暴露 OpenAI SDK 类型。
- 不在 `Tool` 中暴露 Tokio process 类型。
- 不在 Session 接口中暴露 SQLx 行对象。
- 不在插件接口中暴露 Rust 内部结构。

### 5.3 `sagent-model`

职责：

- Provider 配置
- API Key 和 credential resolution
- OpenAI Chat Completions
- OpenAI Responses API
- Anthropic Messages API
- Gemini adapter
- 流式 SSE 解析
- 重试、超时和错误分类
- Model fallback
- Usage 和成本统计

统一请求模型：

```rust
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub reasoning: Option<ReasoningConfig>,
    pub metadata: RequestMetadata,
}
```

不要让 Agent Loop 直接调用 `reqwest` 或某个 SDK。Agent 只依赖
`ModelProvider` trait。

### 5.4 `sagent-runtime`

这是系统的业务核心，负责：

- 创建 Agent Session
- 管理 Turn
- 执行模型循环
- 调用工具
- 发布事件
- 保存消息
- 处理取消
- 触发压缩
- 管理子代理

核心循环：

```rust
pub async fn run_turn(
    session: &mut AgentSession,
    input: UserInput,
    ctx: TurnContext,
) -> Result<TurnResult, RuntimeError> {
    session.append_user_message(input.message).await?;

    loop {
        ctx.cancel_token().throw_if_cancelled()?;

        let request = session.build_model_request().await?;
        let response = ctx.model.stream(request, ctx.cancel_token()).await?;

        let assistant = consume_model_stream(response, &ctx.events).await?;
        session.append_assistant_message(assistant.clone()).await?;

        if assistant.tool_calls.is_empty() {
            return Ok(TurnResult::completed(assistant));
        }

        let results = ctx.tools.execute_batch(
            session.tool_context(),
            assistant.tool_calls,
            ctx.cancel_token(),
        ).await?;

        for result in results {
            session.append_tool_result(result).await?;
        }

        session.ensure_context_budget().await?;
    }
}
```

### 5.5 `sagent-tools`

工具系统应采用显式注册，而不是扫描源码或依赖模块导入副作用。

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), RegistryError>;
    pub fn definition(&self, name: &str) -> Option<ToolDefinition>;
    pub fn definitions(&self, policy: &ToolPolicy) -> Vec<ToolDefinition>;
    pub async fn execute(
        &self,
        name: &str,
        ctx: ToolContext,
        args: Value,
    ) -> Result<ToolOutput, ToolError>;
}
```

P0 工具：

- `terminal`
- `process`
- `read_file`
- `write_file`
- `patch_file`
- `search_files`

P1 工具：

- `session_search`
- `delegate_task`
- `clarify`
- `skill_view`
- `skill_manage`

工具 Definition 应保持稳定：

```rust
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub risk: RiskLevel,
    pub capabilities: Vec<Capability>,
}
```

工具 schema 在一个 Session 内应固定。配置变化不应隐式重建正在运行的会话工具集。

### 5.6 `sagent-session`

职责：

- Session CRUD
- Message append/read
- Session resume
- Session branch
- Session metadata
- Tool call 记录
- Usage 记录
- FTS 搜索
- 压缩 continuation lineage

推荐使用 SQLite：

- 本地部署简单
- WAL 支持并发读取
- FTS5 适合会话搜索
- 迁移和备份成本低
- 不需要额外数据库服务

推荐接口：

```rust
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    async fn create_session(&self, input: CreateSession) -> Result<Session, StoreError>;
    async fn get_session(&self, id: &SessionId) -> Result<Option<Session>, StoreError>;
    async fn append_message(&self, message: PersistedMessage) -> Result<(), StoreError>;
    async fn list_messages(&self, id: &SessionId) -> Result<Vec<PersistedMessage>, StoreError>;
    async fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>, StoreError>;
    async fn branch(&self, id: &SessionId) -> Result<Session, StoreError>;
}
```

第一版不追求兼容当前项目的数据库。Sagent 应使用自己的 schema 版本和 migration。

### 5.7 `sagent-config`

配置文件建议为 YAML 或 TOML。推荐：

- 用户配置使用 YAML，适合复杂嵌套配置。
- 内部强类型使用 `serde` 结构体。
- 环境变量只用于 secret 和显式运行时覆盖。
- 非 secret 行为设置不放入 `.env`。

推荐目录：

```text
~/.sagent/
├── config.yaml
├── secrets.env
├── state.db
├── logs/
├── skills/
├── plugins/
├── cache/
└── runs/
```

配置原则：

- 启动时解析配置并生成不可变 RuntimeConfig。
- 运行中配置变化默认只影响新 Session。
- 显式 reload 才允许重载外部服务。
- 正在进行的 Turn 不动态替换 Provider 或 ToolSet。

示例：

```yaml
model:
  provider: openrouter
  model: anthropic/claude-sonnet
  base_url: https://openrouter.ai/api/v1

agent:
  max_iterations: 40
  max_context_tokens: 100000
  tool_timeout_seconds: 300

tools:
  enabled:
    - terminal
    - file
  approval:
    mode: dangerous

terminal:
  cwd: .
  allowed_roots: []

session:
  database: state.db

runtime:
  max_concurrent_sessions: 32
```

### 5.8 `sagent-security`

安全模型应独立成 crate，而不是散落在工具实现中。

核心对象：

```rust
pub struct SecurityContext {
    pub session_id: SessionId,
    pub principal: Principal,
    pub workspace: WorkspacePolicy,
    pub capabilities: CapabilitySet,
    pub approval: ApprovalPolicy,
}
```

至少实现：

- 文件路径 canonicalization
- allowed roots
- 禁止路径和 symlink 检查
- 命令风险分类
- 命令审批
- 环境变量过滤
- 子进程环境隔离
- 工具超时和输出限制
- 插件权限声明
- Secret 不进入普通日志

风险等级：

```rust
pub enum RiskLevel {
    ReadOnly,
    WorkspaceWrite,
    ProcessExecution,
    NetworkAccess,
    CredentialAccess,
    Admin,
}
```

## 6. 并发模型

Sagent 建议采用 Tokio + Actor/Channel，而不是大量共享可变状态。

### 6.1 Session Actor

每个活跃 Session 由一个逻辑 Actor 管理：

```text
SessionHandle
    -> mpsc::Sender<SessionCommand>

SessionActor
    -> 当前消息历史
    -> 当前 Turn 状态
    -> ToolSet 快照
    -> CancellationToken
    -> Event Publisher
```

这样可以保证：

- 同一 Session 的消息顺序稳定。
- 不会同时运行两个冲突 Turn。
- `/stop`、审批响应、用户输入可以统一排队。
- 避免 Python 版本中大量锁、线程和 event loop 桥接问题。

### 6.2 Supervisor

`AgentSupervisor` 管理：

- Session Actor 生命周期
- 最大并发 Session 数
- 子代理并发限制
- 空闲 Session 回收
- 任务取消
- 运行状态查询

### 6.3 取消语义

所有长任务都接收 `CancellationToken`：

- Model HTTP 请求
- SSE stream
- Terminal process
- Browser action
- Plugin RPC
- MCP request
- Subagent turn
- Scheduler job

取消必须做到：

1. 立即向调用方返回取消状态。
2. 尽可能终止底层任务。
3. 不破坏 SQLite 一致性。
4. 将未完成 Turn 保存为可恢复状态。

## 7. Prompt Cache 与上下文模型

Prompt Cache 是 Sagent 的核心约束，不是优化项。

### 7.1 Session Prompt Snapshot

每个 Session 创建时生成：

```rust
pub struct PromptSnapshot {
    pub system_prompt: String,
    pub tool_definitions: Vec<ToolDefinition>,
    pub provider: ProviderId,
    pub model: String,
    pub version: String,
    pub hash: String,
}
```

之后每一轮只追加：

- user message
- assistant message
- tool result
- 必要的压缩 continuation

不允许普通命令在会话中间偷偷改变：

- system prompt
- tool definitions
- provider
- model routing
- memory prompt block

### 7.2 Context Budget

```rust
pub struct ContextBudget {
    pub max_tokens: u32,
    pub reserved_output_tokens: u32,
    pub compression_threshold: u32,
    pub current_estimate: u32,
}
```

触发顺序：

1. 估算当前上下文。
2. 保留 system prompt、最近用户输入和未完成 tool pair。
3. 在独立压缩流程中生成 summary。
4. 创建 continuation session 或替换允许压缩的历史段。
5. 保持消息 role 和 tool call 配对完整。

压缩是唯一允许改变历史上下文的特殊操作。

## 8. Tool 系统

### 8.1 显式注册

不要扫描 Rust 源码，也不要依赖模块导入副作用。使用构造器装配：

```rust
pub fn build_builtin_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(TerminalTool::new())).unwrap();
    registry.register(Arc::new(ReadFileTool::new())).unwrap();
    registry.register(Arc::new(WriteFileTool::new())).unwrap();
    registry.register(Arc::new(PatchFileTool::new())).unwrap();
    registry
}
```

这比自动扫描更容易：

- 做静态分析
- 做启动失败检查
- 控制暴露面
- 生成文档
- 做权限审核
- 保证 schema 稳定

### 8.2 ToolSet

```rust
pub struct ToolSet {
    pub id: String,
    pub tools: Vec<String>,
    pub policy: ToolPolicy,
}
```

建议预置：

- `safe`
- `file`
- `terminal`
- `research`
- `coding`
- `full`

ToolSet 在 Session 启动时解析并冻结。

### 8.3 工具执行策略

默认顺序执行。只有满足以下条件才并行：

- 工具显式声明无写入副作用。
- 工具之间没有资源冲突。
- 参数路径不重叠。
- Provider 允许并行 tool calls。
- 取消和错误传播语义已定义。

文件写入、终端命令和具有外部副作用的工具默认不并行。

## 9. Provider 设计

### 9.1 Provider Adapter 层

统一接口：

```rust
pub struct ProviderProfile {
    pub id: ProviderId,
    pub display_name: String,
    pub api_mode: ApiMode,
    pub base_url: Url,
    pub capabilities: ProviderCapabilities,
    pub auth: AuthSpec,
}
```

`ProviderProfile` 只描述能力，不负责保存 Session 状态。

### 9.2 Provider 优先级

第一阶段：

1. OpenAI Chat Completions
2. OpenRouter
3. Ollama
4. LM Studio

第二阶段：

1. Anthropic Messages
2. Gemini
3. Azure OpenAI
4. AWS Bedrock

第三阶段：

1. Codex/Responses 风格 API
2. 自定义 Provider Plugin
3. 本地推理后端

### 9.3 HTTP 技术方案

推荐：

- `reqwest`：HTTP Client
- `eventsource-stream` 或自定义 SSE parser：流式事件
- `serde` / `serde_json`：请求响应模型
- `tower`：timeout、retry、限流 middleware
- `url`：URL 解析
- `secrecy`：Secret 包装
- `zeroize`：敏感数据清理

Provider 错误统一分类：

```rust
pub enum ModelError {
    Authentication,
    RateLimited { retry_after: Option<Duration> },
    Timeout,
    ContextTooLarge,
    InvalidRequest(String),
    ProviderUnavailable,
    StreamInterrupted,
    Cancelled,
    Unknown(String),
}
```

## 10. Session 数据库

Sagent 使用独立 schema，不兼容当前项目数据库。

建议初始表：

```sql
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    profile TEXT NOT NULL,
    source TEXT NOT NULL,
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    system_prompt_hash TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    status TEXT NOT NULL,
    parent_session_id TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    role TEXT NOT NULL,
    content_json TEXT NOT NULL,
    tool_call_id TEXT,
    created_at INTEGER NOT NULL,
    FOREIGN KEY(session_id) REFERENCES sessions(id)
);

CREATE TABLE tool_calls (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    message_id INTEGER,
    tool_name TEXT NOT NULL,
    arguments_json TEXT NOT NULL,
    result_json TEXT,
    status TEXT NOT NULL,
    started_at INTEGER,
    ended_at INTEGER
);

CREATE TABLE usage (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_read_tokens INTEGER,
    cache_write_tokens INTEGER,
    estimated_cost_micros INTEGER
);
```

实现选型：

- `sqlx` + SQLite
- `sqlx::migrate!` 管理迁移
- WAL 模式
- `foreign_keys = ON`
- busy timeout
- 事务封装 append message + tool state
- FTS5 在第二阶段加入

## 11. API 与协议

### 11.1 JSON-RPC

CLI、TUI、Desktop 和外部客户端统一使用 JSON-RPC 2.0。

传输方式：

- Stdio：本地 TUI 和插件
- WebSocket：Desktop、Web UI、实时事件
- HTTP：管理 API、健康检查、Webhook

核心请求：

```text
session.create
session.list
session.get
session.resume
session.delete
prompt.submit
session.interrupt
session.branch
session.compress
tool.approval.respond
commands.catalog
config.get
health.get
```

核心事件：

```text
gateway.ready
session.created
turn.started
message.delta
message.reasoning_delta
tool.start
tool.progress
tool.approval_required
tool.complete
turn.completed
turn.failed
turn.interrupted
session.updated
```

事件应使用统一 Envelope：

```json
{
  "jsonrpc": "2.0",
  "method": "message.delta",
  "params": {
    "session_id": "sess_123",
    "turn_id": "turn_456",
    "seq": 12,
    "delta": "hello"
  }
}
```

要求：

- 每个事件包含 `session_id` 和 `turn_id`。
- 每个事件包含单调递增 `seq`。
- 客户端可以检测丢事件。
- 请求和事件支持 correlation ID。
- 事件 payload 使用稳定 JSON schema。

### 11.2 协议版本

```json
{
  "protocol": "sagent.rpc",
  "version": "1",
  "features": ["streaming", "approval", "session_resume"]
}
```

协议版本和 Runtime 版本分开管理。

## 12. Plugin 与 MCP

### 12.1 插件策略

不要从第一天开始设计 Rust 动态库 ABI。Rust ABI 不稳定，跨版本发布成本高。

推荐外部进程插件：

```text
Sagent Runtime
      |
      | JSON-RPC over stdin/stdout
      v
Plugin Process
```

插件 manifest：

```yaml
id: example-tools
version: 0.1.0
protocol: sagent.plugin.v1
entrypoint: ./plugin
permissions:
  - network
  - workspace_read
tools:
  - example_search
```

插件进程具备：

- 独立崩溃边界
- 独立依赖
- 独立升级周期
- 清晰权限声明
- 与 Python、Rust、Go 等语言无关

后续可以增加：

- WASM 插件，用于纯计算和受限逻辑
- MCP Server 适配器
- 官方 Rust Plugin SDK

### 12.2 MCP

MCP 应作为外部协议适配层，不应污染核心 Tool trait。

```text
MCP Client
  -> MCP Server
  -> Remote Tool Definition
  -> Sagent Tool Adapter
```

MCP 工具需要经过：

- tool name namespace
- schema 校验
- timeout
- output limit
- server allowlist
- 网络权限检查
- 动态工具集冻结策略

## 13. Skills 与 Memory

### 13.1 Skills

Skill 是静态说明和流程资产，不应默认变成动态系统提示重建。

建议格式：

```text
~/.sagent/skills/<name>/SKILL.md
```

加载策略：

1. Session 创建时读取已启用 Skill。
2. 生成 Prompt Snapshot。
3. Session 内不自动改变已加载 Skill。
4. Skill 安装或更新默认下一 Session 生效。
5. 显式 `--reload` 才允许创建新 Prompt Snapshot。

### 13.2 Memory

Memory 分为三层：

1. Session Transcript：当前会话完整历史。
2. Local Memory：本地结构化 facts、preferences、notes。
3. External Memory：可选的远程 Provider。

不要让 Memory Provider 直接修改主会话消息。Memory 的召回内容必须有清晰边界：

```text
<memory-context>
...
</memory-context>
```

并在发送模型前做清理和长度限制。

## 14. CLI、TUI 与 Desktop

### 14.1 CLI

建议使用 `clap`：

```text
sagent chat
sagent run --prompt "..."
sagent session list
sagent session resume <id>
sagent tools list
sagent config get model.provider
sagent plugin list
sagent doctor
```

CLI 分为两类：

- 无状态命令：`doctor`、`config`、`tools list`
- Runtime 命令：`chat`、`run`、`session resume`

### 14.2 TUI

第一阶段可以继续使用 TypeScript/Ink 或其他前端，只连接 Sagent JSON-RPC。

Rust 不需要承担 UI 渲染职责。TUI 只负责：

- Transcript 展示
- 输入编辑
- 工具进度
- 审批交互
- Session picker
- Slash command

### 14.3 Desktop

Desktop 使用 WebSocket/HTTP 连接 `sagentd`。不要把 Agent Loop 嵌入 Electron。

推荐：

```text
Desktop UI
    -> WebSocket
    -> sagentd
    -> Agent Runtime
```

这样可以让 CLI、TUI、Desktop 共享完全相同的 Runtime 行为。

## 15. Gateway

Gateway 应独立于 Agent Core，只负责：

- 接收平台消息
- 身份认证和权限
- Session 路由
- 发送响应和事件
- 平台媒体转换

平台适配器接口：

```rust
#[async_trait::async_trait]
pub trait ChannelAdapter: Send + Sync {
    fn id(&self) -> &str;
    async fn start(&self, sink: EventSink) -> Result<(), ChannelError>;
    async fn send(&self, target: Target, message: OutgoingMessage) -> Result<(), ChannelError>;
    async fn stop(&self) -> Result<(), ChannelError>;
}
```

第一阶段只实现：

- Webhook
- 内置 HTTP API

之后再加入 Telegram、Discord、Slack 等平台。平台逻辑不能反向耦合 Agent Loop。

## 16. Scheduler

Scheduler 放到 P2，第一版可以使用系统 Cron 或外部调用。

Rust 版本实现时：

- Job 数据存 SQLite。
- 每个 Job 有独立 Session。
- Job 运行有 wall-clock timeout。
- Job 结果通过 EventSink 投递。
- 不把定时任务消息写入普通交互 Session。
- 使用数据库 claim 防止多个进程重复执行。

## 17. 技术选型

| 领域 | 推荐选型 | 原因 |
| --- | --- | --- |
| Async Runtime | Tokio | 生态成熟、取消和网络支持完整 |
| HTTP Client | reqwest | TLS、流式响应、代理支持成熟 |
| HTTP Server | axum | Tokio 原生、类型安全、组合简单 |
| WebSocket | axum + tokio-tungstenite | 与 HTTP Server 统一 |
| Serialization | serde / serde_json | Rust 生态标准 |
| Config YAML | serde_yaml 或 `yaml_serde` | 兼容用户可读配置 |
| CLI | clap | 子命令、帮助、类型解析成熟 |
| TUI | ratatui 或独立 TypeScript TUI | Rust 或前端均可，优先协议解耦 |
| Database | SQLite + sqlx | 本地优先、迁移、异步访问 |
| Search | SQLite FTS5 | 不增加独立搜索服务 |
| Concurrency | Tokio channels + actor | 降低共享状态复杂度 |
| Cancellation | tokio-util CancellationToken | 统一取消语义 |
| Retry | tower / backoff | Provider 和网络重试 |
| Error | thiserror + anyhow | 库错误结构化，边界错误易处理 |
| Logging | tracing + tracing-subscriber | 结构化日志和 span |
| Metrics | metrics 或 OpenTelemetry | 后续可选，不默认外发 telemetry |
| Secret | secrecy + zeroize | 降低密钥意外泄露风险 |
| Process | tokio::process | 异步进程、取消、超时 |
| File Watching | notify | 配置和 Skill 变化监听 |
| Plugin RPC | JSON-RPC over stdio | 跨语言、隔离、可调试 |
| Tests | cargo test + insta + wiremock | 单元、快照、HTTP 行为测试 |
| Fuzzing | cargo-fuzz | parser、schema、路径安全测试 |
| Supply Chain | cargo-deny + cargo-audit | 许可证和漏洞检查 |

依赖策略：

- 生产依赖尽量减少。
- 锁定 `Cargo.lock`。
- CI 执行 `cargo deny check`。
- 外部二进制必须校验版本和 hash。
- 不把不必要的 telemetry 放入默认安装。

## 18. 分阶段实施路线

### Phase 0：项目基础与协议设计

目标：建立新项目骨架，不实现完整 Agent。

内容：

- 初始化 Cargo Workspace。
- 建立 `sagent-types`。
- 定义 Message、ToolCall、ToolDefinition、ModelEvent。
- 定义 JSON-RPC request/event schema。
- 定义错误码和协议版本。
- 建立 tracing 日志。
- 建立 CI、格式化、lint、audit。
- 建立 `~/.sagent` 路径规则。

验收：

- `cargo check --workspace` 通过。
- JSON schema 可生成和校验。
- Stdio JSON-RPC echo server 可运行。
- Linux、macOS、Windows CI 基础编译通过。

### Phase 1：基础设施 P0

目标：完成可运行的本地 Runtime 基础。

内容：

- `sagent-config`
- `sagent-session`
- SQLite migration
- `sagent-api`
- Session Actor
- Stdio transport
- 基础 CLI
- Session create/list/get/resume

验收：

- 进程重启后可以恢复 Session。
- 两个 Session 并行运行互不污染。
- SQLite 写入事务完整。
- 停止进程不会留下损坏数据库。
- RPC 客户端可以订阅 Session 事件。

### Phase 2：Agent Core P0

目标：完成模型对话和工具调用闭环。

内容：

- OpenAI Chat Completions Provider
- SSE streaming
- Tool Registry
- Terminal Tool
- File Tools
- Tool result 回填
- iteration budget
- timeout 和 cancellation
- Prompt Snapshot
- Usage tracking

验收：

- 能完成普通问答。
- 能完成一次工具调用。
- 能完成多轮工具调用。
- 工具失败后模型可以继续处理。
- `/stop` 可以停止模型和工具。
- 工具输出不会无限增长。
- 系统提示和工具集合在 Session 内保持稳定。

### Phase 3：可靠性与安全 P0/P1

目标：让 Runtime 可以长期运行。

内容：

- 错误分类
- Provider retry/fallback
- Rate limit backoff
- Context Budget
- Context Compression
- 危险命令审批
- 路径安全
- 子进程环境隔离
- 崩溃恢复
- 结构化诊断日志

验收：

- Provider 超时不会阻塞其他 Session。
- Provider 429 按策略退避。
- 上下文超限可以压缩并继续对话。
- 危险命令无法绕过审批。
- symlink 和路径穿越测试通过。
- Cancel、timeout、process crash 都有明确 Session 状态。

### Phase 4：Subagent、Skills 与 MCP P1/P2

目标：形成可扩展的 Agent 平台。

内容：

- Subagent Session
- 子代理并发限制
- 父子 Session lineage
- Skills 目录和加载器
- Skill 命令
- MCP Client
- 外部进程 Plugin Host
- Plugin manifest 和权限

验收：

- 子代理拥有独立 Session。
- 子代理不能默认访问父代理全部工具。
- 子代理完成结果可以回传父代理。
- 插件崩溃不导致 Runtime 崩溃。
- MCP schema 错误不会破坏整个 ToolSet。
- Skill 更新默认不改变当前 Session Prompt Snapshot。

### Phase 5：HTTP/WebSocket、TUI 与 Desktop P2

目标：支持多前端接入。

内容：

- HTTP API
- WebSocket event stream
- `gateway.ready`
- reconnect 和 event sequence
- TUI 客户端
- Desktop 客户端适配
- Session picker
- Approval UI

验收：

- WebSocket 断线后可重连。
- 客户端能检测事件丢失。
- 多个客户端不会重复执行同一个 Turn。
- UI 只依赖 RPC，不依赖 Rust 内部模块。

### Phase 6：Gateway、Scheduler 与生态 P2/P3

目标：扩展产品边界。

内容：

- Webhook Channel
- Telegram
- Discord
- Scheduler
- Cron Job
- Memory Provider
- Browser Automation
- Docker/SSH Terminal Backend
- Plugin Registry

验收：

- Channel 消息正确路由到 Session。
- Gateway 控制命令可以中断运行中 Agent。
- Scheduler 不重复执行 Job。
- 外部 Memory 故障不影响主 Agent。
- Browser 和远程 Terminal 均有独立权限和超时。

## 19. 测试策略

### 19.1 单元测试

覆盖：

- Message role 交替
- Tool call 配对
- Prompt hash
- Tool schema 冻结
- Context budget
- Retry 分类
- 路径安全
- 命令风险分类
- 配置 merge
- Session 状态机

### 19.2 协议测试

为 JSON-RPC 建立 conformance suite：

- request/response 配对
- notification
- error code
- event sequence
- reconnect
- cancellation
- unknown method
- malformed payload

### 19.3 Provider 测试

使用 mock HTTP server 验证：

- 普通响应
- 流式响应
- tool call streaming
- 429
- 401
- 500
- malformed SSE
- provider timeout
- context too large

### 19.4 集成测试

每个测试使用临时 `SAGENT_HOME`：

```text
temp/
└── .sagent/
    ├── config.yaml
    └── state.db
```

不允许测试写入真实用户目录。

### 19.5 Golden 测试

适合固定协议和数据模型，不适合冻结动态模型目录。

可以冻结：

- JSON-RPC event shape
- SQLite migration result
- 错误码
- Tool schema contract

不要冻结：

- 当前 Provider model 列表
- 当前插件数量
- 当前 Skill 数量
- 配置版本常量

### 19.6 Fuzz 测试

重点 fuzz：

- SSE parser
- JSON-RPC parser
- Tool arguments
- YAML config
- Patch parser
- Search query sanitizer
- 文件路径输入
- Plugin manifest

## 20. 迁移和发布策略

Sagent 是独立项目，不做旧项目运行时兼容。但可以提供可选导入工具：

```text
sagent import hermes-jsonl <path>
sagent import hermes-sessions --source <db>
```

导入工具必须：

- 独立于 Runtime 核心。
- 只读源数据库。
- 先写临时数据库。
- 校验完成后原子替换。
- 报告无法转换的字段。

发布建议：

- 单一 `sagent` CLI binary。
- 可选 `sagentd` daemon binary。
- macOS、Linux、Windows 构建。
- 后续提供 Homebrew、AUR、安装脚本和 GitHub Release。
- 不要求用户安装 Python 或 Node 才能使用核心 Runtime。

## 21. 第一版推荐最小目录

如果希望尽快开始，不需要一开始创建所有 crate。最小版本为：

```text
sagent/
├── Cargo.toml
├── crates/
│   ├── sagent-types/
│   ├── sagent-core/
│   ├── sagent-model/
│   ├── sagent-tools/
│   ├── sagent-session/
│   ├── sagent-runtime/
│   └── sagent-api/
└── bins/
    └── sagent/
```

第一批实现顺序：

1. `sagent-types`
2. `sagent-session`
3. `sagent-model`
4. `sagent-tools`
5. `sagent-runtime`
6. `sagent-api`
7. `sagent` CLI

不要先实现：

- Plugin
- Gateway
- Browser
- Memory
- Scheduler
- Desktop

## 22. 关键决策总结

| 问题 | Sagent 决策 |
| --- | --- |
| 是否兼容旧 Python 内部结构 | 否 |
| 是否兼容旧 SQLite schema | 否，提供独立导入工具 |
| 核心架构 | 模块化单体 |
| 并发模型 | Tokio + Session Actor + Channels |
| Agent 状态 | Session Actor 管理，SQLite 持久化 |
| Provider 接口 | Rust trait，隔离 HTTP 实现 |
| Tool 注册 | 显式注册，不扫描源码 |
| 插件 ABI | 外部进程 JSON-RPC，暂不做 Rust ABI |
| 前端连接 | JSON-RPC over stdio/WebSocket/HTTP |
| 数据库 | SQLite + WAL + SQLx |
| Prompt Cache | Session Prompt Snapshot，不中途重建 |
| 配置更新 | 默认下一 Session 生效 |
| 安全 | 独立 security crate，能力和审批模型 |
| 第一版 Provider | OpenAI 兼容接口 |
| 第一版工具 | Terminal + File |
| 第一版 UI | CLI，TUI 后续接入 |

## 23. 推荐的第一条开发主线

```text
Week 1: Workspace + types + JSON-RPC schema
Week 2: config + paths + tracing
Week 3: SQLite session store
Week 4: OpenAI-compatible provider
Week 5: streaming model events
Week 6: terminal/file tools
Week 7: runtime turn loop
Week 8: CLI chat + session resume
Week 9: cancellation + approval
Week 10: context budget + error handling
```

第一阶段完成后，Sagent 应能做到：

```bash
sagent chat
sagent run --prompt "读取当前目录并总结项目结构"
sagent session list
sagent session resume <session-id>
```

这比先实现完整 Gateway、插件市场或桌面应用更能验证架构是否正确。
