# CODEBUDDY.md This file provides guidance to CodeBuddy when working with code in this repository.

## 项目概述

sagent 是一个模块化的本地优先 AI Agent Runtime，采用 Rust 从零重写，不兼容 Python 版 Hermes Agent 的配置/数据库格式。目标为单二进制、低资源占用、生产级部署。

核心设计原则：协议优先（Runtime/CLI/TUI/Desktop/插件通过稳定协议交互）、核心窄腰（Agent Loop/Session/Tool/Provider/Event 是核心，具体能力放边缘）、本地优先（数据存 `~/.sagent`）、Prompt Cache 安全（Session 内系统提示和工具集合保持稳定）、取消优先（所有长任务可取消）、安全默认（文件/终端/插件默认受限）、模块化单体优先（内部解耦，初期不拆微服务）。

完整架构设计：[plans/sagent-rust-architecture.md](plans/sagent-rust-architecture.md)
Phase 0 实施指南：[plans/sagent-phase0-implementation-guide.md](plans/sagent-phase0-implementation-guide.md)

## 常用命令

```bash
# 构建 workspace 全部 crate
cargo build --workspace

# 构建（release 模式）
cargo build --release

# 运行所有测试
cargo test --workspace

# 运行单个测试
cargo test <test_name>

# 仅编译特定 crate
cargo build -p sagent-types

# 代码检查（clippy pedantic，零警告要求）
cargo clippy -- -D warnings

# 代码格式化检查
cargo fmt --check

# 依赖审计
cargo deny check && cargo audit

# 运行集成测试
cargo test --test integration -- config_roundtrip

# 生成文档
cargo doc --open
```

## 核心架构

### Cargo Workspace 结构

项目采用自底向上的依赖分层，底层 crate 不得反向依赖 binary crate：

```text
sagent-types                         # 零 IO 依赖，纯数据模型
    ^
sagent-core                          # 核心 trait 定义（ModelProvider/Tool/MemoryProvider）
    ^
sagent-session / sagent-config / sagent-security / sagent-model / sagent-tools
    ^                                  # 基础设施层，各自独立
sagent-runtime                       # 业务编排核心（Agent Loop/Session Actor/Turn 执行）
    ^
sagent-api / sagent-gateway / sagent-scheduler
    ^                                  # 接入层
sagent-cli / sagentd                 # 二进制入口
```

目录布局（严格对齐 `plans/sagent-rust-architecture.md` 第 4 节）：

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
│   ├── sagent-types/           # 共享数据模型（Message/ToolCall/ModelEvent）
│   ├── sagent-core/            # 核心 trait 接口（ModelProvider/Tool/MemoryProvider）
│   ├── sagent-runtime/         # Agent 业务编排（Turn Loop/Session Actor/EventBus）
│   ├── sagent-model/           # Provider 适配（OpenAI/Anthropic/Gemini）
│   ├── sagent-tools/           # 工具系统（显式注册 + Registry + 安全策略）
│   ├── sagent-session/         # SQLite 持久化（FTS5/WAL/migration）
│   ├── sagent-config/          # 配置与路径（YAML + .env 仅密钥）
│   ├── sagent-security/        # 安全模型（路径/命令审批/权限声明）
│   ├── sagent-plugin/          # 插件运行时（外部进程 JSON-RPC）
│   ├── sagent-mcp/             # MCP 客户端适配
│   ├── sagent-api/             # JSON-RPC 协议层（stdio/WebSocket/HTTP）
│   ├── sagent-gateway/         # 多平台消息网关
│   ├── sagent-scheduler/       # 定时任务调度
│   ├── sagent-terminal/        # 终端/PTY 执行后端
│   ├── sagent-memory/          # Memory Provider 实现
│   └── sagent-observability/   # 结构化日志/指标/诊断
│
├── bins/
│   ├── sagent/                 # CLI 主程序（clap + REPL + TUI）
│   ├── sagentd/                # Daemon 进程（HTTP/WebSocket 服务）
│   └── sagent-plugin-host/     # 外部插件宿主进程
│
├── adapters/
│   ├── provider-openai/        # OpenAI 兼容 Provider
│   ├── provider-anthropic/     # Anthropic Messages Provider
│   ├── provider-gemini/        # Google Gemini Provider
│   ├── channel-webhook/        # Webhook 平台适配器
│   └── browser/                # CDP 浏览器驱动
│
├── protocols/
│   ├── jsonrpc/                # JSON-RPC schema 定义
│   ├── schemas/                # 协议 schema
│   └── conformance/            # 协议一致性测试
│
├── migrations/
│   └── sqlite/                 # SQLite migration 脚本
│
├── skills/
│   └── coding/                 # 内置 Skill 示例
│
└── tests/
    ├── integration/            # 集成测试
    ├── conformance/            # 协议一致性测试
    ├── fixtures/               # 测试 fixture 数据
    └── golden/                 # Golden 测试快照
```

### 依赖约束

- `sagent-types`：不依赖 Runtime、数据库或 HTTP，纯数据结构 + serde。
- `sagent-core`：不依赖 SQLite、CLI、Gateway，只定义 trait 接口。
- `sagent-model`：不依赖 Gateway，只负责 Provider HTTP 适配。
- `sagent-tools`：不依赖具体 CLI，工具通过 trait 注册。
- `sagent-api`：只负责协议和请求分发，不包含业务逻辑。
- `sagent-runtime`：负责业务编排，不依赖具体 transport 实现。
- Binary crate：负责启动、信号处理和依赖装配。

### 核心设计锚点（贯穿所有阶段）

1. **Prompt Cache 是神圣的**：system prompt + tool schema 在 Session 创建时生成 `PromptSnapshot`（含 hash），整个生命周期内 byte-stable。之后每轮只追加 user/assistant/tool_result/压缩 continuation。不允许普通命令中途改变 system prompt、tool definitions、provider、model routing、memory prompt block。上下文压缩是唯一允许改变历史的特殊操作，采用写时复制。
2. **窄腰原则**：新增模型工具的优先级——扩展现有代码 → CLI 命令 + skill → service-gated tool（`check_fn`）→ 插件 → MCP catalog → 新 core tool（最后手段）。
3. **交替不变量**：消息序列绝不允许连续两条同角色消息，绝不在循环中注入合成 user 消息。
4. **行为契约测试 > 快照测试**：断言不变量（交替、缓存前缀哈希、路径拒绝），不 freeze 枚举计数/版本字面量（避免 change-detector 测试）。

### Agent 核心数据流

Agent 的 Turn Loop（`sagent-runtime`）是系统最核心路径：

1. **构建 Prompt Snapshot**（Session 创建时）：加载 Skill 内容 → 构建 system prompt → 构建 tool schema（仅包含通过可用性检查的工具）→ 计算 snapshot hash。Snapshot 在 Session 生命周期内 byte-stable。
2. **Turn 执行循环**（`IterationBudget` 控制最大轮次）：
   - 调用 `ModelProvider::stream()` 发送请求（Agent 只依赖 trait，不直接调用 HTTP）
   - 消费 `ModelStream`，通过 `EventBus` 发布事件
   - 响应分叉：纯文本 → 结束 Turn 返回；含 tool_calls → `ToolRegistry::execute_batch()` 执行 → 追加 tool_result 继续循环
   - 错误分类处理：`RateLimited` → 指数退避重试；`ContextTooLarge` → 触发压缩后重试；`Authentication`/`InvalidRequest` → 不重试
3. **Post-turn**：追加消息到 Session Store → Memory sync（后台 fire-and-forget）→ 更新 Usage 统计

### 并发模型（Session Actor）

采用 Tokio + Actor/Channel，不使用大量共享可变状态。每个活跃 Session 由独立 Actor 管理：

```text
SessionHandle
    -> mpsc::Sender<SessionCommand>

SessionActor
    -> 当前消息历史 / Turn 状态 / ToolSet 快照
    -> CancellationToken / Event Publisher
```

- 同一 Session 的消息顺序稳定，不会同时运行两个冲突 Turn
- `/stop`、审批响应、用户输入统一排队
- `AgentSupervisor` 管理 Session Actor 生命周期、最大并发数、空闲回收、子代理并发限制、任务取消、运行状态查询
- 所有长任务接收 `CancellationToken`：Model HTTP 请求、SSE stream、Terminal process、Browser action、Plugin RPC、MCP request、Subagent turn、Scheduler job
- 取消语义：立即返回取消状态、尽可能终止底层任务、不破坏 SQLite 一致性、未完成 Turn 保存为可恢复状态

### Prompt Cache 与上下文模型

Prompt Cache 是核心约束，不是优化项。

`PromptSnapshot`（Session 创建时生成）：
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

`ContextBudget`：
```rust
pub struct ContextBudget {
    pub max_tokens: u32,
    pub reserved_output_tokens: u32,
    pub compression_threshold: u32,
    pub current_estimate: u32,
}
```

压缩触发顺序：估算上下文 → 保留 system prompt + 最近用户输入 + 未完成 tool pair → 独立压缩流程生成 summary → 创建 continuation session 或替换允许压缩的历史段 → 保持 role 和 tool call 配对完整。

### Provider 设计

统一 trait 接口，Agent Loop 不直接依赖 HTTP 实现。`ProviderProfile` 只描述能力，不保存 Session 状态：

```rust
pub struct ProviderProfile {
    pub id: ProviderId,
    pub display_name: String,
    pub api_mode: ApiMode,
    pub base_url: Url,
    pub capabilities: ProviderCapabilities,
    pub auth: AuthSpec,
}

#[async_trait::async_trait]
pub trait ModelProvider: Send + Sync {
    fn id(&self) -> &ProviderId;
    async fn complete(&self, request: ModelRequest, cancel: CancellationToken) -> Result<ModelResponse, ModelError>;
    async fn stream(&self, request: ModelRequest, cancel: CancellationToken) -> Result<ModelStream, ModelError>;
}
```

Provider 优先级分三阶段：
- 第一阶段：OpenAI Chat Completions → OpenRouter → Ollama → LM Studio
- 第二阶段：Anthropic Messages → Gemini → Azure OpenAI → AWS Bedrock
- 第三阶段：Codex/Responses API → 自定义 Provider Plugin → 本地推理后端

HTTP 技术栈：`reqwest`（Client）、`eventsource-stream` 或自定义 SSE parser（流式）、`tower`（timeout/retry/限流 middleware）、`url`（URL 解析）、`secrecy`（Secret 包装）、`zeroize`（敏感数据清理）。

错误统一分类：
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

### Tool 系统

采用显式注册，不扫描源码或依赖模块导入副作用：

```rust
pub fn build_builtin_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(TerminalTool::new()));
    registry.register(Arc::new(ReadFileTool::new()));
    registry.register(Arc::new(WriteFileTool::new()));
    registry.register(Arc::new(PatchFileTool::new()));
    registry
}
```

工具 Definition：
```rust
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub risk: RiskLevel,
    pub capabilities: Vec<Capability>,
}
```

- P0 工具：terminal、process、read_file、write_file、patch_file、search_files
- P1 工具：session_search、delegate_task、clarify、skill_view、skill_manage
- 风险等级：ReadOnly / WorkspaceWrite / ProcessExecution / NetworkAccess / CredentialAccess / Admin
- 预置 ToolSet：`safe`、`file`、`terminal`、`research`、`coding`、`full`，在 Session 启动时解析并冻结
- 默认顺序执行；只有显式声明无写入副作用的工具才允许并行
- 文件写入、终端命令和具有外部副作用的工具默认不并行
- MCP 工具作为外部协议适配层，不污染核心 Tool trait，需经过 namespace/schema 校验/timeout/output limit/server allowlist/网络权限检查

### 安全模型

独立 `sagent-security` crate，不在工具实现中散落安全逻辑：

- **文件安全**：路径 canonicalization、allowed roots、禁止路径和 symlink 检查
- **命令安全**：风险分类、审批 guardrail（默认拦截 `rm -rf /` 等危险操作）
- **进程安全**：环境变量过滤、子进程环境隔离、超时和输出限制
- **凭证安全**：`secrecy::SecretString`（Drop 清零）、`zeroize` 清理敏感数据；日志自动脱敏
- **插件安全**：权限声明、独立崩溃边界、网络权限检查
- **secrets 加密**：AES-GCM + Argon2 KDF，密钥从 `.env`/keychain 派生

### 配置体系

- **路径**：`~/.sagent/`（`SAGENT_HOME` env 可覆盖），Windows 使用 `%LOCALAPPDATA%\sagent`
- **目录结构**：`config.yaml` / `secrets.env` / `state.db` / `logs/` / `skills/` / `plugins/` / `cache/` / `runs/`
- **配置格式**：用户配置使用 YAML（`serde_yaml`），内部使用强类型 `serde` 结构体
- **硬性规则**：行为配置在 `config.yaml`，密钥在环境变量/`.env`——非 secret 行为设置不放入 `.env`
- **生效策略**：启动时解析生成不可变 `RuntimeConfig`；运行中配置变化默认只影响新 Session；显式 reload 才允许重载外部服务；正在进行的 Turn 不动态替换 Provider 或 ToolSet
- **Profile**：每个 Profile 是独立岛屿，拥有独立 Config 实例，互不继承

示例配置：
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
  enabled: [terminal, file]
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

### Session 数据库

Sagent 使用独立 schema，不兼容旧项目数据库。实现选型：`sqlx` + SQLite、`sqlx::migrate!` 管理迁移、WAL 模式、`foreign_keys = ON`、busy timeout、事务封装 append message + tool state、FTS5 在第二阶段加入。

核心表：
```sql
CREATE TABLE sessions (
    id TEXT PRIMARY KEY, profile TEXT NOT NULL, source TEXT NOT NULL,
    model TEXT NOT NULL, provider TEXT NOT NULL,
    system_prompt_hash TEXT NOT NULL, started_at INTEGER NOT NULL,
    ended_at INTEGER, status TEXT NOT NULL,
    parent_session_id TEXT, metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL, role TEXT NOT NULL,
    content_json TEXT NOT NULL, tool_call_id TEXT,
    created_at INTEGER NOT NULL,
    FOREIGN KEY(session_id) REFERENCES sessions(id)
);

CREATE TABLE tool_calls (
    id TEXT PRIMARY KEY, session_id TEXT NOT NULL, message_id INTEGER,
    tool_name TEXT NOT NULL, arguments_json TEXT NOT NULL,
    result_json TEXT, status TEXT NOT NULL,
    started_at INTEGER, ended_at INTEGER
);

CREATE TABLE usage (
    id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
    turn_id TEXT NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL,
    input_tokens INTEGER, output_tokens INTEGER,
    cache_read_tokens INTEGER, cache_write_tokens INTEGER,
    estimated_cost_micros INTEGER
);
```

Session Store 接口：
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

### 协议与 API

统一使用 JSON-RPC 2.0。传输方式：Stdio（本地 TUI 和插件）、WebSocket（Desktop/Web UI/实时事件）、HTTP（管理 API/健康检查/Webhook）。

核心请求方法：
```text
session.create / session.list / session.get / session.resume / session.delete
prompt.submit / session.interrupt / session.branch / session.compress
tool.approval.respond / commands.catalog / config.get / health.get
```

核心事件通知：
```text
gateway.ready / session.created / turn.started
message.delta / message.reasoning_delta
tool.start / tool.progress / tool.approval_required / tool.complete
turn.completed / turn.failed / turn.interrupted / session.updated
```

事件 Envelope 规范：
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

要求：每个事件包含 `session_id` + `turn_id` + 单调递增 `seq`，客户端可检测丢事件，支持 correlation ID，payload 使用稳定 JSON schema。

协议版本声明：
```json
{ "protocol": "sagent.rpc", "version": "1", "features": ["streaming", "approval", "session_resume"] }
```
协议版本与 Runtime 版本分开管理。

### Skills 与 Memory

**Skills**：静态说明和流程资产，格式为 `~/.sagent/skills/<name>/SKILL.md`。加载策略：(1) Session 创建时读取已启用 Skill；(2) 生成 Prompt Snapshot；(3) Session 内不自动改变已加载 Skill；(4) Skill 安装或更新默认下一 Session 生效；(5) 显式 `--reload` 才允许创建新 Prompt Snapshot。

**Memory 三层模型**：(1) Session Transcript：当前会话完整历史；(2) Local Memory：本地结构化 facts/preferences/notes；(3) External Memory：可选的远程 Provider。Memory 召回内容用 `<memory-context>...</memory-context>` 包裹，有清晰边界，发送模型前做清理和长度限制。Memory Provider 不直接修改主会话消息。

### 插件与 MCP

第一版不做 Rust 动态库 ABI（Rust ABI 不稳定）。采用外部进程 JSON-RPC over stdin/stdout，插件 manifest：
```yaml
id: example-tools
version: 0.1.0
protocol: sagent.plugin.v1
entrypoint: ./plugin
permissions: [network, workspace_read]
tools: [example_search]
```
插件进程具备：独立崩溃边界、独立依赖、独立升级周期、清晰权限声明、跨语言无关。后续增加 WASM 插件（纯计算/受限逻辑）、MCP Server 适配器、官方 Rust Plugin SDK。

MCP 作为外部协议适配层，不污染核心 Tool trait。MCP 工具需经过 tool name namespace、schema 校验、timeout、output limit、server allowlist、网络权限检查、动态工具集冻结策略。

### CLI、TUI 与 Desktop

CLI（`clap`）分两类：
- 无状态命令：`doctor`、`config get`、`tools list`、`plugin list`
- Runtime 命令：`chat`、`run --prompt "..."`、`session list`、`session resume <id>`

TUI 第一阶段可复用 TypeScript/Ink 前端连接 Sagent JSON-RPC，Rust 不承担 UI 渲染。TUI 职责：Transcript 展示、输入编辑、工具进度、审批交互、Session picker、Slash command。

Desktop 通过 WebSocket/HTTP 连接 `sagentd`，不把 Agent Loop 嵌入 Electron：`Desktop UI → WebSocket → sagentd → Agent Runtime`。CLI、TUI、Desktop 共享完全相同的 Runtime 行为。

### Gateway

Gateway 独立于 Agent Core，只负责接收平台消息、身份认证和权限、Session 路由、发送响应和事件、平台媒体转换。平台逻辑不能反向耦合 Agent Loop。

```rust
#[async_trait::async_trait]
pub trait ChannelAdapter: Send + Sync {
    fn id(&self) -> &str;
    async fn start(&self, sink: EventSink) -> Result<(), ChannelError>;
    async fn send(&self, target: Target, message: OutgoingMessage) -> Result<(), ChannelError>;
    async fn stop(&self) -> Result<(), ChannelError>;
}
```

第一阶段只实现 Webhook + 内置 HTTP API，之后加入 Telegram/Discord/Slack 等。

### Scheduler

放到 P2，第一版使用系统 Cron 或外部调用。Rust 实现时：Job 数据存 SQLite、每个 Job 有独立 Session、wall-clock timeout、结果通过 EventSink 投递、不把定时任务消息写入普通交互 Session、使用数据库 claim 防止多进程重复执行。

### 技术选型

| 领域 | 选型 | 原因 |
|------|------|------|
| Async Runtime | Tokio | 生态成熟、取消和网络支持完整 |
| HTTP Client | reqwest | TLS、流式响应、代理支持成熟 |
| HTTP Server | axum | Tokio 原生、类型安全、组合简单 |
| WebSocket | axum + tokio-tungstenite | 与 HTTP Server 统一 |
| Serialization | serde / serde_json | Rust 生态标准 |
| Config | serde_yaml | 兼容用户可读 YAML 配置 |
| CLI | clap | 子命令、帮助、类型解析成熟 |
| TUI | ratatui 或独立 TS TUI | 优先协议解耦 |
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

依赖策略：生产依赖尽量减少、锁定 `Cargo.lock`、CI 执行 `cargo deny check`、外部二进制校验版本和 hash、不把不必要的 telemetry 放入默认安装。

### 分阶段实施路线

| 阶段 | 内容 | 验收标准 |
|------|------|----------|
| **0 项目基础** | Workspace 骨架、`sagent-types`、JSON-RPC schema、tracing 日志、CI/CD 跨平台矩阵、`~/.sagent` 路径规则 | `cargo check --workspace` 通过；JSON schema 可生成校验；Stdio JSON-RPC echo server 可运行；三平台 CI 编译通过 |
| **1 基础设施** | `sagent-config`、`sagent-session`（SQLite WAL/migration）、`sagent-api`、Session Actor、Stdio transport、基础 CLI、Session CRUD | 进程重启可恢复 Session；两个 Session 并行互不污染；SQLite 事务完整；停止进程不损坏数据库；RPC 客户端可订阅 Session 事件 |
| **2 Agent 内核** | OpenAI Provider + SSE streaming、Tool Registry + Terminal/File 工具、Turn Loop、iteration budget、Prompt Snapshot、Usage tracking | 能完成问答、一次工具调用、多轮工具调用；工具失败后模型可继续；`/stop` 可停止模型和工具；工具输出不会无限增长；系统提示和工具集合在 Session 内稳定 |
| **3 可靠性与安全** | 错误分类、Provider retry/fallback、Rate limit backoff、Context Budget/Compression、危险命令审批、路径安全、子进程隔离、崩溃恢复 | Provider 超时不阻塞其他 Session；429 按策略退避；上下文超限可压缩继续；危险命令无法绕过审批；symlink/路径穿越测试通过；Cancel/timeout/crash 都有明确 Session 状态 |
| **4 子代理/Skills/MCP** | Subagent Session、子代理并发限制、父子 lineage、Skills 加载器、MCP Client、外部进程 Plugin Host、Plugin manifest | 子代理独立 Session、不默认访问父代理全部工具、结果可回传；插件崩溃不导致 Runtime 崩溃；MCP schema 错误不破坏 ToolSet；Skill 更新不改变当前 Session Snapshot |
| **5 前端接入** | HTTP API、WebSocket event stream、`gateway.ready`、reconnect/event sequence、TUI 客户端、Desktop 适配 | WebSocket 断线可重连；客户端可检测事件丢失；多客户端不重复执行同一 Turn；UI 只依赖 RPC |
| **6 生态扩展** | Webhook/TG/Discord Gateway、Scheduler、Memory Provider、Browser Automation、Docker/SSH Backend、Plugin Registry | Channel 消息正确路由到 Session；Gateway 控制命令可中断 Agent；Scheduler 不重复执行 Job；外部 Memory 故障不影响主 Agent；Browser 和远程 Terminal 有独立权限和超时 |

**当前状态**：阶段 0 初期，Workspace 已配置（`Cargo.toml`），crates 目录待创建。下一步按 [plans/sagent-phase0-implementation-guide.md](plans/sagent-phase0-implementation-guide.md) 执行。

### 第一条开发主线（Phase 0-2）

```text
Week 1: Workspace + sagent-types + JSON-RPC schema
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

第一阶段完成后应能执行：`sagent chat` / `sagent run --prompt "..."` / `sagent session list` / `sagent session resume <id>`

### 关键设计决策速查

| 问题 | 决策 |
|------|------|
| 是否兼容旧 Python 内部结构 | 否 |
| 是否兼容旧 SQLite schema | 否，提供独立导入工具 |
| 核心架构 | 模块化单体（初期不拆微服务） |
| 并发模型 | Tokio + Session Actor + Channels |
| Agent 状态 | Session Actor 管理，SQLite 持久化 |
| Provider 接口 | Rust trait，隔离 HTTP 实现 |
| Tool 注册 | 显式注册，不扫描源码 |
| 插件 ABI | 外部进程 JSON-RPC，暂不做 Rust ABI |
| 前端连接 | JSON-RPC over stdio/WebSocket/HTTP |
| 数据库 | SQLite + WAL + sqlx |
| Prompt Cache | Session Prompt Snapshot，不中途重建 |
| 配置更新 | 默认下一 Session 生效 |
| LLM Client | 手写 HTTP（reqwest），不依赖社区 SDK |
| 第一版 Provider | OpenAI 兼容接口 |
| 第一版工具 | Terminal + File |
| 用户可见错误信息 | 中文（`thiserror` 消息、Display、日志/CLI 输出） |
| 日志 | `tracing` 5 级体系，携带 session_id/provider/tool_name 等结构化字段 |

### 测试策略

- 单元测试：覆盖 Message role 交替、Tool call 配对、Prompt hash、Tool schema 冻结、Context budget、Retry 分类、路径安全、命令风险分类、配置 merge、Session 状态机
- 协议测试（conformance suite）：request/response 配对、notification、error code、event sequence、reconnect、cancellation、unknown method、malformed payload
- Provider 测试（mock HTTP server）：普通响应、流式响应、tool call streaming、429、401、500、malformed SSE、provider timeout、context too large
- 集成测试：每个测试使用临时 `SAGENT_HOME`（temp/.sagent/config.yaml + state.db），不写入真实用户目录
- Golden 测试：冻结 JSON-RPC event shape、SQLite migration result、错误码、Tool schema contract；不冻结动态模型列表/插件数量/配置版本常量
- Fuzz 测试：SSE parser、JSON-RPC parser、Tool arguments、YAML config、Patch parser、Search query sanitizer、文件路径输入、Plugin manifest

## 编码规范

### 文件头注释

每个 `.rs` 源文件必须以标准文件头注释开头，包含作者和创建日期，**不需要写变更记录（change 信息）**：

```rust
//! Sagent - <模块简要描述>
//!
//! Author: SongZQ
//! Created: 2026-08-07
```

对于 `lib.rs` 和 `mod.rs`，使用 `//!` 模块级文档注释；对于普通 `.rs` 文件，使用 `//!` 或 `//` 均可，但必须保持一致性。

### 注释语言

**所有注释必须使用中文（简体中文）**，包括：

- 模块级文档注释（`//!`）
- 函数/方法文档注释（`///`）
- 行内注释（`//`）
- 结构体/枚举/trait 的文档注释
- 测试中的说明注释

唯一例外：Commit message 使用英文。

### 函数/方法注释规范

1. **简单功能**：仅需一行简要描述，无需 `# Arguments`、`# Returns` 等额外标记。
   ```rust
   /// 将 SessionId 转换为字符串表示。
   pub fn to_string(&self) -> String { ... }

   /// 创建新的 SessionActor，使用给定的配置和存储后端。
   pub fn new(config: Arc<RuntimeConfig>, store: Arc<dyn SessionStore>) -> Self { ... }
   ```

2. **复杂功能**（多参数、多分支逻辑、涉及错误处理）：按照 Google 代码规范，包含：
   - 功能描述（必填）
   - `# Arguments`：每个参数的说明
   - `# Returns`：返回值说明
   - `# Errors`：可能抛出的错误（如有）
   - `# Panics`：可能 panic 的场景（如有）
   - `# Examples`：使用示例（推荐）

   ```rust
   /// 根据给定的消息列表和工具定义构建 ModelRequest。
   ///
   /// # Arguments
   ///
   /// * `messages` - 对话历史消息列表，按时间顺序排列。
   /// * `tools` - 当前可用的工具定义集合。
   /// * `config` - 模型请求的配置参数。
   ///
   /// # Returns
   ///
   /// 返回构建好的 `ModelRequest`，可直接传给 `ModelProvider::stream()`。
   ///
   /// # Examples
   ///
   /// ```ignore
   /// let request = build_model_request(&messages, &tools, &config)?;
   /// let stream = provider.stream(request, cancel_token).await?;
   /// ```
   pub fn build_model_request(
       messages: &[Message],
       tools: &[ToolDefinition],
       config: &RequestConfig,
   ) -> Result<ModelRequest, BuildError> { ... }
   ```

### 测试中的注释

测试模块和测试函数也必须使用中文注释：

```rust
#[cfg(test)]
mod tests {
    //! 消息序列化与反序列化的单元测试。

    /// 验证 Message 可以正确地进行 JSON round-trip 序列化。
    #[test]
    fn test_message_roundtrip() { ... }
}
```

### 日志中的语言

- **用户可见的错误信息**：使用中文（`thiserror` 的 `#[error("...")]` 消息、`Display` 实现、通过日志/TUI/CLI 暴露给用户的文本）
- **内部开发日志**（`tracing::debug!`/`trace!`）：可使用英文
- **结构化日志字段名**：使用英文（`session_id`、`provider`、`tool_name` 等）
