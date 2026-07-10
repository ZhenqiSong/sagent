# 🦀 Hermes Agent Rust 重构 — 架构设计与实施计划

> 最后更新: 2026-07-09
> 基于: Python 版 v0.18.2 架构分析
> 目标: 单二进制、低资源占用、生产级部署
> 部署场景: 单机 + 容器化 + 高并发 Gateway

---

## 目录

1. [总体设计哲学](#一总体设计哲学)
2. [Crate 架构](#二crate-架构)
3. [核心 Trait 体系](#三核心-trait-体系)
4. [核心数据流](#四核心数据流)
5. [工具系统](#五工具系统)
6. [Provider 适配层](#六provider-适配层)
7. [Agent 核心循环](#七agent-核心循环)
8. [Gateway 架构](#八gateway-架构)
9. [并发模型](#九并发模型)
10. [配置系统](#十配置系统)
11. [安全设计](#十一安全设计)
12. [实施路线图](#十二实施路线图)
13. [技术选型与决策记录](#十三技术选型与决策记录)

---

## 一、总体设计哲学

### 1.1 对齐 Python 版的核心原则

| 原则 | Rust 对应策略 |
|------|--------------|
| **Per-conversation prompt caching 不可触碰** | `Arc<ConversationState>` 不可变前缀，压缩走写时复制 |
| **Core 是窄腰，能力在边缘** | `CoreTrait` 最小化，新能力通过 CLI command + Skill / Plugin / MCP 扩展 |
| **自注册工具** | `linkme` crate 实现编译期工具注册 |
| **Profile 独立岛屿** | 每个 Profile 拥有独立的 `Config` 实例，不继承 |
| **门控工具 (check_fn)** | `Tool::check_availability()` → 返回 false 的工具不在 schema 中出现 |

### 1.2 Rust 特有的设计约束

- **所有权模型**：多读单写 → `Arc<RwLock<T>>` 用于共享状态
- **异步生态**：`tokio` 作为统一异步运行时
- **零成本抽象**：充分使用 trait + generic，避免不必要的动态派发
- **错误处理**：`thiserror` 定义错误类型，`anyhow` 用于应用级错误传播
- **工作区模型**：单一 Cargo Workspace，5 个 crate，共享 `Cargo.lock`

---

## 二、Crate 架构

### 2.1 Workspace 结构

```
hermes-agent/
├── Cargo.toml                          # [workspace] 定义
├── Cargo.lock                          # 全 workspace 共享
├── crates/
│   ├── hermes-common/                  # ★ 共享类型 + 纯函数（零 IO 依赖）
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── types.rs                # Message, Role, ContentBlock, TokenUsage
│   │       ├── error.rs                # HermesError 枚举
│   │       ├── config.rs               # Config / Profile 数据结构
│   │       └── utils.rs                # 纯工具函数
│   │
│   ├── hermes-core/                    # ★ 核心库（最重要的 crate）
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                  # 公开 re-export
│   │       ├── agent.rs                # Agent 结构体 + AgentConfig
│   │       ├── conversation.rs         # 对话循环（核心逻辑）
│   │       ├── context.rs              # 上下文管理 + Token 计数 + 压缩
│   │       ├── compression.rs          # 上下文压缩器
│   │       ├── caching.rs              # Prompt caching 优化
│   │       ├── retry.rs                # 重试策略（指数退避）
│   │       ├── budget.rs               # IterationBudget 预算控制
│   │       ├── security.rs             # 安全沙箱（路径、命令）
│   │       ├── display.rs              # 终端输出/Spinner
│   │       ├── provider/               # Provider 抽象层
│   │       │   ├── mod.rs              # LLMProvider trait
│   │       │   ├── openai.rs           # OpenAI 适配器
│   │       │   ├── anthropic.rs        # Anthropic 适配器（thinking / cache_control）
│   │       │   ├── gemini.rs           # Gemini 适配器
│   │       │   ├── bedrock.rs          # AWS Bedrock 适配器（后续）
│   │       │   ├── factory.rs          # ProviderFactory + 模型路由
│   │       │   └── credential.rs       # API Key 池 + 轮转
│   │       ├── tool/                   # 工具系统
│   │       │   ├── mod.rs              # Tool trait + ToolRegistry
│   │       │   ├── dispatch.rs         # 工具调度（并发、超时、重试）
│   │       │   ├── terminal.rs         # 终端工具
│   │       │   ├── file.rs             # 文件操作工具
│   │       │   ├── web.rs              # Web 搜索/抓取
│   │       │   ├── browser.rs          # 浏览器自动化
│   │       │   ├── memory.rs           # 记忆工具
│   │       │   ├── skill.rs            # 技能工具
│   │       │   ├── delegate.rs         # 任务委托工具
│   │       │   ├── todo.rs             # 待办工具
│   │       │   └── vision.rs           # 视觉/图像工具
│   │       ├── session/                # 会话存储
│   │       │   ├── mod.rs              # SessionStore trait
│   │       │   ├── sqlite.rs           # SQLite + FTS5 实现
│   │       │   └── postgres.rs         # PostgreSQL 实现（可选）
│   │       ├── memory/                 # 记忆系统
│   │       │   ├── mod.rs              # MemoryProvider trait
│   │       │   ├── builtin.rs          # 内置 SQLite 记忆
│   │       │   └── manager.rs          # MemoryManager 多 Provider 管理
│   │       └── skill/                  # 技能系统
│   │           ├── mod.rs              # Skill 结构体
│   │           ├── loader.rs           # 技能加载（文件/URL）
│   │           ├── manager.rs          # SkillManager 管理
│   │           └── template.rs         # 模板变量替换 + inline shell
│   │
│   ├── hermes-cli/                     # ★ CLI 二进制 + TUI
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs                 # 入口（clap 命令解析）
│   │       ├── commands/               # 子命令
│   │       │   ├── mod.rs
│   │       │   ├── run.rs              # hermes（默认 REPL）
│   │       │   ├── gateway.rs          # hermes gateway
│   │       │   ├── tools.rs            # hermes tools
│   │       │   ├── setup.rs            # hermes setup
│   │       │   ├── cron.rs             # hermes cron
│   │       │   └── mcp.rs              # hermes mcp
│   │       ├── display.rs              # 终端输出格式化（colored + termimad）
│   │       ├── repl.rs                 # 交互式 REPL（prompt_toolkit 等价物）
│   │       └── tui/                    # TUI 界面
│   │           ├── mod.rs
│   │           ├── app.rs              # TUI 应用状态
│   │           ├── chat_view.rs        # 聊天界面
│   │           ├── input.rs            # 输入框
│   │           └── widgets.rs          # 自定义组件
│   │
│   ├── hermes-gateway/                 # ★ 消息网关
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── runner.rs               # GatewayRunner 主循环
│   │       ├── session.rs              # 多用户会话管理
│   │       ├── cache.rs                # Agent 实例 LRU 缓存
│   │       ├── hooks.rs                # 网关级 Hook
│   │       └── platforms/              # 平台适配器
│   │           ├── mod.rs              # PlatformAdapter trait
│   │           ├── telegram.rs         # teloxide
│   │           ├── discord.rs          # serenity
│   │           ├── slack.rs            # slack_morphism
│   │           ├── whatsapp.rs
│   │           ├── signal.rs
│   │           ├── matrix.rs
│   │           ├── wechat.rs           # 微信/企微
│   │           ├── feishu.rs           # 飞书
│   │           └── api_server.rs       # REST API Server
│   │
│   └── hermes-plugin/                  # ★ 插件 SDK（供第三方插件开发者使用）
│       ├── Cargo.toml
│       └── src/
│           └── lib.rs                  # Plugin trait + PluginHook 定义
│
├── configs/
│   └── config.yaml.example
├── skills/                             # 内置技能（Markdown 文件）
├── plugins/                            # 内置插件
├── tests/                              # 集成测试
│   ├── integration/                    # 集成测试（testcontainers, wiremock）
│   └── e2e/                            # 端到端测试
├── benchmarks/                         # 基准测试（criterion）
├── docs/
│   └── rust-impl-plan.md               # 本文档
├── .github/
│   └── workflows/
│       ├── ci.yml                      # CI：test + lint + clippy
│       └── release.yml                 # 发布：build + package + publish
├── rustfmt.toml
├── clippy.toml
└── Dockerfile
```

### 2.2 Crate 依赖层级

```
hermes-cli ─────────────┐
                        │
hermes-gateway ─────────┼──► hermes-core ──► hermes-common
                        │
                        │
hermes-plugin ◄─────────┘  (SDK，不依赖 core，仅定义 trait)
```

- **`hermes-common`**：零 IO 依赖，纯数据结构和工具函数，所有 crate 可安全依赖
- **`hermes-core`**：主逻辑层，包含 Agent/Provider/Tool/Session/Memory/Skill，不依赖 CLI/Gateway
- **`hermes-cli`**：二进制入口，依赖 core 提供所有功能
- **`hermes-gateway`**：消息网关，依赖 core 的 Agent + Session，同时定义 `PlatformAdapter` trait
- **`hermes-plugin`**：轻量 SDK，仅定义 `Plugin` trait，供第三方插件 crate 依赖

### 2.3 为什么不拆多 Workspace

| 因素 | 单 workspace ✅ | 多 workspace ❌ |
|------|----------------|-----------------|
| **耦合度** | core/common 紧密耦合，一起迭代 | 独立发布不需要 |
| **版本一致性** | 共享 `Cargo.lock`，依赖版本统一 | 可能版本漂移 |
| **编译速度** | 共享 `target/`，增量编译快 | 各自编译，重复构建 |
| **重构便利** | 跨 crate 重构一次编译通过 | 逐个 workspace 验证 |
| **发布策略** | 统一版本号（都跟随 Hermes 版本） | 独立版本号 |

---

## 三、核心 Trait 体系

### 3.1 `LLMProvider` — 模型提供商抽象

```rust
/// 统一的 LLM Provider 接口。
///
/// 所有模型提供商（OpenAI、Anthropic、Gemini 等）都实现此 trait。
/// 设计要点：
/// - 使用泛型流类型避免动态派发
/// - 区分完整响应和流式响应
/// - Provider 负责内部格式转换（如 tool schema 的 OpenAI ↔ Anthropic 转换）
/// - 不同 Provider 的 system prompt 位置不同（OpenAI 在消息列表首条，
///   Anthropic 有独立 system 字段，Gemini 用 system_instruction）
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Provider 的唯一标识符（用于日志和选择）
    fn provider_id(&self) -> &'static str;

    /// 发送聊天请求，返回完整响应
    async fn chat(
        &self,
        request: ChatRequest,
    ) -> Result<ChatResponse, HermesError>;

    /// 发送流式聊天请求，返回 SSE 事件流
    async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, HermesError>> + Send>>, HermesError>;

    /// 将标准化 ToolSchema 转换为 Provider 特定格式
    fn convert_tools(&self, tools: &[ToolSchema]) -> Vec<serde_json::Value>;

    /// 组合系统提示词到消息列表（不同 Provider 位置不同）
    fn inject_system_prompt(&self, system: &str, messages: &mut Vec<Message>);

    /// Provider 支持的能力标记
    fn capabilities(&self) -> ProviderCapabilities;
}

/// Provider 能力标记
pub struct ProviderCapabilities {
    pub supports_streaming: bool,
    pub supports_thinking: bool,
    pub supports_vision: bool,
    pub supports_tool_use: bool,
    pub supports_prompt_caching: bool,
    pub supports_parallel_tool_calls: bool,
}
```

### 3.2 `Tool` — 工具抽象

```rust
/// 工具 trait，所有内置工具和插件工具都必须实现。
///
/// 工具通过 #[linkme::distributed_slice] 宏在编译期自动注册到全局注册表，
/// 无需运行时扫描，无需手动维护工具列表。
///
/// # 生命周期
/// 1. **注册**：编译期通过 linkme 收集所有 `#[register_tool]`
/// 2. **可用性检查**：每次构建 tool schema 时调用 `check_availability()`
///    返回 false 的工具不出现在 LLM 的工具列表中（节省 token）
/// 3. **执行**：`execute(args)` 在 tokio 任务中并发执行
///
/// # 示例
/// ```rust,ignore
/// #[register_tool(name = "web_search", toolset = "web")]
/// struct WebSearchTool;
///
/// #[async_trait]
/// impl Tool for WebSearchTool {
///     fn name(&self) -> &'static str { "web_search" }
///     fn toolset(&self) -> &'static str { "web" }
///
///     fn schema(&self) -> ToolSchema {
///         ToolSchema::function("web_search", "Search the web for information")
///             .param("query", json!({
///                 "type": "string",
///                 "description": "The search query"
///             }))
///             .required("query")
///     }
///
///     fn check_availability(&self) -> bool {
///         std::env::var("SEARCH_API_KEY").is_ok()
///     }
///
///     async fn execute(
///         &self,
///         args: serde_json::Value,
///         ctx: &ToolContext,
///     ) -> Result<ToolResult, HermesError> {
///         let query = args["query"].as_str().unwrap_or_default();
///         // ... 执行搜索 ...
///         Ok(ToolResult::success(results))
///     }
/// }
/// ```
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具唯一名称（用于 LLM 工具调用匹配）
    fn name(&self) -> &'static str;

    /// 工具所属的工具集（如 "web", "terminal", "file", "browser"）
    fn toolset(&self) -> &'static str;

    /// 工具的 JSON Schema 定义（发送给 LLM 的函数描述）
    fn schema(&self) -> ToolSchema;

    /// 门控检查：返回 true 时工具才出现在 schema 中。
    /// 用于有前置条件依赖的工具（如需要 API key、特定 OS、已安装的软件）
    /// 默认始终可用。
    fn check_availability(&self) -> bool {
        true
    }

    /// 默认超时时间（可通过 AgentConfig 覆盖）
    fn default_timeout(&self) -> Duration {
        Duration::from_secs(60)
    }

    /// 执行工具，接收 JSON 参数和上下文，返回结果
    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, HermesError>;
}

/// 工具执行上下文
pub struct ToolContext {
    pub session_id: String,
    pub working_dir: PathBuf,
    pub env_vars: HashMap<String, String>,
}

/// 工具执行结果
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    pub metadata: ToolResultMetadata,
}

pub struct ToolResultMetadata {
    pub tool_name: String,
    pub duration: Duration,
    pub truncated: bool,
}
```

### 3.3 `SessionStore` — 会话持久化

```rust
/// 会话存储抽象层。默认使用 SQLite + FTS5 全文搜索。
/// 可选 PostgreSQL + pgvector 用于分布式部署。
///
/// 设计要点：
/// - 对齐 Python 版的 hermes_state.py 数据模型
/// - SQLite 使用 WAL 模式支持读写并发
/// - Session 之间完全隔离（不同 profile 不同 store 实例）
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// 创建新会话
    async fn create_session(&self, profile: &str, title: Option<&str>)
        -> Result<Session, HermesError>;

    /// 获取会话详情
    async fn get_session(&self, id: &SessionId)
        -> Result<Option<Session>, HermesError>;

    /// 列出所有会话摘要
    async fn list_sessions(&self)
        -> Result<Vec<SessionSummary>, HermesError>;

    /// 追加一条消息到会话
    async fn append_message(&self, session_id: &SessionId, msg: &Message)
        -> Result<(), HermesError>;

    /// 获取会话的所有消息
    async fn get_messages(&self, session_id: &SessionId)
        -> Result<Vec<Message>, HermesError>;

    /// FTS5 全文搜索会话内容
    async fn search(&self, query: &str, limit: usize)
        -> Result<Vec<SessionSearchResult>, HermesError>;

    /// 删除会话
    async fn delete_session(&self, id: &SessionId)
        -> Result<(), HermesError>;
}
```

### 3.4 `MemoryProvider` — 长期记忆

```rust
/// 记忆提供者抽象。支持多个 Provider 协同工作（本地 SQLite + 云端 API）。
#[async_trait]
pub trait MemoryProvider: Send + Sync {
    fn provider_id(&self) -> &'static str;

    /// 预取与当前查询/上下文相关的记忆
    async fn prefetch(&self, query: &str, limit: usize)
        -> Result<Vec<MemoryEntry>, HermesError>;

    /// 将新记忆写入持久存储（对话结束后异步调用）
    async fn sync(&self, entries: &[MemoryEntry])
        -> Result<(), HermesError>;
}

pub struct MemoryEntry {
    pub content: String,
    pub importance: f32,
    pub created_at: DateTime<Utc>,
    pub source_session: Option<SessionId>,
    pub tags: Vec<String>,
}
```

### 3.5 `PlatformAdapter` — 消息平台接入

```rust
/// 消息平台适配器 trait。每个平台（Telegram, Discord, Slack 等）实现此 trait。
#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    /// 平台标识名
    fn platform_name(&self) -> &'static str;

    /// 连接到平台（WebSocket / long polling）
    async fn connect(&self) -> Result<(), HermesError>;

    /// 断开连接
    async fn disconnect(&self) -> Result<(), HermesError>;

    /// 接收消息的异步流
    async fn receive_messages(&self)
        -> Result<Pin<Box<dyn Stream<Item = Result<IncomingMessage, HermesError>> + Send>>, HermesError>;

    /// 发送文本消息
    async fn send_message(&self, chat_id: &str, content: &str)
        -> Result<(), HermesError>;

    /// 发送附件（图片、文件）
    async fn send_attachment(&self, chat_id: &str, attachment: &Attachment)
        -> Result<(), HermesError>;

    /// 发送 typing 指示器
    async fn send_typing(&self, chat_id: &str)
        -> Result<(), HermesError>;

    /// 平台特定功能标记
    fn features(&self) -> PlatformFeatures;
}
```

### 3.6 `Plugin` — 插件接口

```rust
/// 插件 trait。第三方插件通过动态库（.so/.dylib/.dll）或 WASM 加载。
///
/// 插件可以：
/// - 注册自定义工具
/// - 添加生命周期 Hook
/// - 提供自定义 Provider
#[async_trait]
pub trait Plugin: Send + Sync {
    /// 插件元信息
    fn metadata(&self) -> PluginMetadata;

    /// 注册工具（在插件加载时被调用）
    fn register_tools(&self, registry: &mut Box<dyn PluginToolRegistry>);

    /// 注册生命周期 Hook
    fn hooks(&self) -> Vec<Box<dyn PluginHook>>;

    /// 插件初始化
    async fn on_start(&self) -> Result<(), HermesError>;

    /// 插件清理
    async fn on_shutdown(&self) -> Result<(), HermesError>;
}

pub struct PluginMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
}
```

---

## 四、核心数据流

### 4.1 Agent 对话循环（最核心路径）

```
                    ┌──────────────────────────────────────────────┐
                    │         Agent::run_conversation(input)         │
                    │                                               │
  User Input ──────►│  1. build_turn_context()                      │
                    │     ├─ 加载 Session 消息历史                   │
                    │     ├─ 获取 Memory prefetch（相关记忆）         │
                    │     ├─ 注入启用的 Skill 内容                   │
                    │     ├─ 构建系统提示词                           │
                    │     └─ 构建 tool schema（只包含 check_fn=true） │
                    │                                               │
                    │  2. 应用 Prompt Caching                        │
                    │     └─ 标记不可变前缀（Anthropic cache_control） │
                    │                                               │
                    │  ┌─── 对话循环 (最多 N 轮) ───────────────┐    │
                    │  │                                        │    │
                    │  │  3. provider.chat(messages, tools)     │    │
                    │  │          │                             │    │
                    │  │          ▼                             │    │
                    │  │  4. 响应类型判断                        │    │
                    │  │     ├─ text → break，返回给用户         │    │
                    │  │     └─ tool_calls                       │    │
                    │  │              │                         │    │
                    │  │              ▼                         │    │
                    │  │  5. tool_registry.dispatch(calls)      │    │
                    │  │     ├─ join_all 并发执行多个 tool       │    │
                    │  │     ├─ tokio::time::timeout 超时控制    │    │
                    │  │     ├─ 错误重试（可配置次数）           │    │
                    │  │     └─ 结果截断（防 token 爆炸）        │    │
                    │  │              │                         │    │
                    │  │              ▼                         │    │
                    │  │  6. 追加 tool_result 到 messages        │    │
                    │  │  7. 检查 IterationBudget 是否耗尽      │    │
                    │  │  8. 未耗尽 → 回到步骤 3                │    │
                    │  └────────────────────────────────────────┘    │
                    │                                               │
                    │  9. Post-turn 后处理                          │
                    │     ├─ 追加消息到 Session Store               │
                    │     ├─ 触发 Memory sync（后台任务）            │
                    │     ├─ 触发 Skill 学习（后台任务）             │
                    │     ├─ 更新 Token 使用统计                    │
                    │     └─ 返回最终响应                           │
                    └──────────────────────────────────────────────┘
```

### 4.2 核心数据结构

```rust
/// Agent 配置（创建后不可变，避免 mid-conversation 缓存失效）
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub max_tool_iterations: usize,     // 最大工具调用轮次
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub tool_timeout: Duration,         // 单个工具超时
    pub enabled_toolsets: Vec<String>,  // ["web", "terminal", "file"]
    pub enable_thinking: bool,
    pub thinking_budget: Option<u32>,
    pub profile: Option<String>,
}

/// Agent 实例
pub struct Agent {
    pub id: AgentId,
    pub session_id: SessionId,
    pub config: AgentConfig,
    pub provider: Arc<dyn LLMProvider>,
    pub tool_registry: Arc<ToolRegistry>,
    pub session_store: Arc<dyn SessionStore>,
    pub memory_manager: Arc<MemoryManager>,
    pub skill_manager: Arc<SkillManager>,
    /// 可变对话状态（多读单写）
    pub state: RwLock<ConversationState>,
}

/// 对话可变状态
pub struct ConversationState {
    pub messages: Vec<Message>,
    pub token_usage: TokenUsage,
    pub budget: IterationBudget,
    pub interrupted: Arc<AtomicBool>,
    pub completed: bool,
}
```

### 4.3 标准化数据模型（对齐 Python 版）

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        source: ImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: Option<bool>,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
}

pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub system: Option<String>,
    pub tools: Vec<ToolSchema>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub stream: bool,
    pub thinking: Option<ThinkingConfig>,
}

pub struct ChatResponse {
    pub content: Vec<ContentBlock>,
    pub finish_reason: FinishReason,
    pub usage: TokenUsage,
    /// Provider 原始响应（用于调试）
    pub raw: Option<serde_json::Value>,
}
```

---

## 五、工具系统

### 5.1 编译期注册机制

使用 `linkme` 实现零成本编译期自注册，替代 Python 版的运行时 `import` 扫描：

```rust
// crates/hermes-core/src/tool/mod.rs
use linkme::distributed_slice;

/// 全局分布式切片。所有 `#[register_tool]` 宏标记的工具在编译期
/// 自动收集到此静态切片中，无需运行时扫描，无需手动维护列表。
#[distributed_slice]
pub static BUILTIN_TOOLS: [ToolRegistration] = [..];

pub struct ToolRegistration {
    pub name: &'static str,
    pub toolset: &'static str,
    pub factory: fn() -> Box<dyn Tool>,
}
```

使用声明宏简化注册：

```rust
// 声明宏示例
// #[register_tool(name = "web_search", toolset = "web")]
// struct WebSearchTool;
//
// 展开后自动生成：
// #[linkme::distributed_slice(BUILTIN_TOOLS)]
// static _REG_WEBSEARCH: ToolRegistration = ToolRegistration {
//     name: "web_search",
//     toolset: "web",
//     factory: || Box::new(WebSearchTool),
// };
```

### 5.2 ToolRegistry 实现

```rust
/// 线程安全的工具注册表。
///
/// 生命周期：
/// - 程序启动：`ToolRegistry::init()` 收集所有 `BUILTIN_TOOLS` 注册
/// - 每次 LLM 请求前：`build_schema()` 生成可用工具列表
/// - LLM 返回工具调用后：`dispatch()` 并发执行
pub struct ToolRegistry {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    /// toolset 名称 → 工具名列表
    toolsets: RwLock<HashMap<String, Vec<String>>>,
}

impl ToolRegistry {
    /// 初始化注册表：收集所有编译期注册的工具
    pub fn init() -> Self {
        let mut tools = HashMap::new();
        let mut toolsets: HashMap<String, Vec<String>> = HashMap::new();

        for reg in BUILTIN_TOOLS.iter() {
            let tool: Arc<dyn Tool> = (reg.factory)().into();
            tools.insert(reg.name.to_string(), Arc::clone(&tool));
            toolsets
                .entry(reg.toolset.to_string())
                .or_default()
                .push(reg.name.to_string());
        }

        Self {
            tools: RwLock::new(tools),
            toolsets: RwLock::new(toolsets),
        }
    }

    /// 注册运行时发现的工具（插件提供的工具）
    pub fn register_dynamic(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        let toolset = tool.toolset().to_string();
        self.tools.write().unwrap().insert(name.clone(), Arc::clone(&tool));
        self.toolsets.write().unwrap()
            .entry(toolset)
            .or_default()
            .push(name);
    }

    /// 生成当前可用的工具 schema
    /// - 只包含启用的 toolset 中的工具
    /// - 只包含 check_availability() 返回 true 的工具
    pub fn build_schema(&self, enabled_toolsets: &[String]) -> Vec<ToolSchema> {
        let tools = self.tools.read().unwrap();
        let toolsets = self.toolsets.read().unwrap();

        let enabled: HashSet<&str> = enabled_toolsets
            .iter()
            .flat_map(|ts| toolsets.get(ts).map(|v| v.as_slice()).unwrap_or(&[]))
            .map(|s| s.as_str())
            .collect();

        tools
            .iter()
            .filter(|(name, tool)| {
                enabled.contains(name.as_str()) && tool.check_availability()
            })
            .map(|(_, tool)| tool.schema())
            .collect()
    }

    /// 并发分发工具调用
    pub async fn dispatch(
        &self,
        calls: &[ToolCallRequest],
        timeout: Duration,
        ctx: &ToolContext,
    ) -> Vec<ToolResult> {
        let handles: Vec<_> = calls
            .iter()
            .map(|call| {
                let tool_name = call.name.clone();
                let args = call.arguments.clone();
                let ctx = ctx.clone();
                let tools_lock = self.tools.read().unwrap();
                let tool = tools_lock.get(&tool_name).cloned();
                drop(tools_lock);

                async move {
                    match tool {
                        Some(t) => {
                            let tool_timeout = t.default_timeout().max(timeout);
                            tokio::time::timeout(tool_timeout, t.execute(args, &ctx))
                                .await
                                .unwrap_or_else(|_| {
                                    ToolResult::error(
                                        &tool_name,
                                        "Tool execution timed out",
                                    )
                                })
                                .unwrap_or_else(|e| {
                                    ToolResult::error(&tool_name, &e.to_string())
                                })
                        }
                        None => ToolResult::error(
                            &tool_name,
                            &format!("Unknown tool: {}", tool_name),
                        ),
                    }
                }
            })
            .collect();

        futures::future::join_all(handles).await
    }
}
```

### 5.3 安全门控设计

```rust
/// 每个工具的 check_availability() 方法实现了"门控"模式：
/// - 返回 true  → 工具出现在 schema 中，LLM 可以调用
/// - 返回 false → 工具完全不可见（节省 token，避免 LLM 调用后失败）
///
/// 门控示例：
///
/// HomeAssistant 工具 —— 仅当 HASS_TOKEN 存在时可用：
/// ```rust
/// fn check_availability(&self) -> bool {
///     std::env::var("HASS_TOKEN").is_ok()
/// }
/// ```
///
/// Computer Use 工具 —— 仅 macOS 且 cua-driver 已安装：
/// ```rust
/// fn check_availability(&self) -> bool {
///     cfg!(target_os = "macos") && which::which("cua-driver").is_ok()
/// }
/// ```
///
/// 桌面端专属工具 —— 仅 HERMES_DESKTOP 环境变量存在时可用：
/// ```rust
/// fn check_availability(&self) -> bool {
///     std::env::var("HERMES_DESKTOP").is_ok()
/// }
/// ```
```

---

## 六、Provider 适配层

### 6.1 ProviderFactory 与模型路由

```rust
/// Provider 工厂：根据模型名称自动选择合适的 Provider。
///
/// 路由规则（按优先级）：
/// 1. 精确匹配：model 名直接对应 provider_id
/// 2. 模式匹配：正则匹配模型名前缀
///    如 "gpt-.*" → openai, "claude-.*" → anthropic
/// 3. 默认：回退到配置的 default_provider
pub struct ProviderFactory {
    providers: HashMap<String, Arc<dyn LLMProvider>>,
    routing_rules: Vec<RoutingRule>,
    default_provider: String,
}

struct RoutingRule {
    model_pattern: Regex,
    provider_id: String,
}

impl ProviderFactory {
    /// 根据模型名自动选择合适的 Provider
    pub fn resolve(&self, model: &str) -> Result<Arc<dyn LLMProvider>, HermesError> {
        for rule in &self.routing_rules {
            if rule.model_pattern.is_match(model) {
                return self.providers.get(&rule.provider_id)
                    .cloned()
                    .ok_or_else(|| HermesError::provider_not_found(&rule.provider_id));
            }
        }
        self.providers.get(&self.default_provider)
            .cloned()
            .ok_or_else(|| HermesError::no_provider_for_model(model))
    }
}
```

### 6.2 多 API Key 轮转

```rust
/// API Key 池：支持多个 key 轮转，实现：
/// - 负载均衡：Round-robin 分配请求
/// - 自动故障转移：某个 key 失败后自动切换
/// - 速率限制感知：key 触发 rate limit 后冷却
pub struct CredentialPool {
    credentials: Mutex<VecDeque<CredentialState>>,
    cooldown: Duration,
}

struct CredentialState {
    key: String,
    base_url: Option<String>,
    fail_count: AtomicU32,
    cooldown_until: Mutex<Option<Instant>>,
}

impl CredentialPool {
    /// 获取下一个可用的凭证。自动跳过冷却中的 key
    pub fn next(&self) -> Option<String> {
        let now = Instant::now();
        let mut creds = self.credentials.lock().unwrap();
        for _ in 0..creds.len() {
            let cred = creds.pop_front().unwrap();
            let in_cooldown = cred.cooldown_until.lock().unwrap()
                .map_or(false, |t| now < t);
            if !in_cooldown {
                let key = cred.key.clone();
                creds.push_back(cred);
                return Some(key);
            }
            creds.push_back(cred);
        }
        None
    }

    /// 标记凭证失败，触发冷却
    pub fn mark_failed(&self, key: &str) { /* ... */ }
}
```

### 6.3 Provider 特定适配要点

| Provider | 关键差异 | 适配策略 |
|----------|---------|---------|
| **OpenAI** | tool schema 标准 JSON Schema / Responses API | 默认格式，最简实现 |
| **Anthropic** | 独立 `system` 字段 / `cache_control` / `thinking` block | `inject_system_prompt` 特殊处理 / Beta header |
| **Gemini** | `system_instruction` 属性 / `safety_settings` / 非标准 tool 格式 | OpenAPI → Gemini 格式互转层 |
| **Bedrock** | AWS IAM 认证 / Converse API / 流式 Token 不同 | 独立认证流程 |
| **Ollama** | 本地 REST API / 仅有部分 model 支持 tool_use | `capabilities()` 严格限制 |

---

## 七、Agent 核心循环

### 7.1 对话循环实现

```rust
impl Agent {
    /// 执行一次用户对话轮次。
    ///
    /// 这是整个系统的核心路径，设计要点：
    /// 1. **不可变前缀**：system prompt + tool schema 在循环外构建，循环内不变
    ///    以保证 prompt caching 不被破坏
    /// 2. **中断安全**：通过 AtomicBool 支持 Ctrl+C 中断
    /// 3. **错误分类**：区分可重试/不可重试/速率限制错误，分别处理
    /// 4. **预算控制**：IterationBudget 防止无限循环
    ///
    /// # 示例
    /// ```rust,ignore
    /// let agent = Agent::new(config, provider, tool_registry, ...).await?;
    /// let response = agent.run_conversation("帮我搜索最新的 Rust 新闻").await?;
    /// println!("{}", response);
    /// ```
    pub async fn run_conversation(&self, user_input: &str) -> Result<String, HermesError> {
        // 1. 构建 turn 上下文（在循环外，保证 prompt caching）
        let turn_ctx = self.build_turn_context(user_input).await?;

        // 2. 将上下文推入消息历史
        {
            let mut state = self.state.write().unwrap();
            state.messages.extend(turn_ctx.messages);
            state.budget.reset(turn_ctx.tool_count);
        }

        // 3. 对话循环
        loop {
            // 3.1 检查中断
            if self.is_interrupted() {
                break;
            }

            // 3.2 检查预算
            if !self.state.read().unwrap().budget.can_continue() {
                break;
            }

            // 3.3 调用 Provider
            let request = self.build_chat_request().await?;
            let response = match self.provider.chat(request).await {
                Ok(resp) => resp,
                Err(e) => {
                    // 错误分类与重试
                    let classified = classify_api_error(&e);
                    if classified.can_retry {
                        self.retry_with_backoff(classified).await?;
                        continue;
                    }
                    return Err(e);
                }
            };

            // 3.4 处理响应
            match self.process_response(response).await? {
                TurnOutcome::Text(text) => {
                    // 最终文本响应 → 结束循环
                    self.finalize_turn(&text).await?;
                    return Ok(text);
                }
                TurnOutcome::ToolCalls(calls) => {
                    // 工具调用 → 执行后继续循环
                    let results = self.tool_registry.dispatch(
                        &calls,
                        self.config.tool_timeout,
                        &self.tool_context(),
                    ).await;
                    self.append_tool_results(calls, results);
                    // 继续循环
                }
                TurnOutcome::CompressionNeeded => {
                    // 上下文超限 → 压缩后重试
                    self.compress_context().await?;
                    continue;
                }
            }
        }
        Ok("Conversation interrupted".to_string())
    }

    /// 构建本轮对话的上下文（不可变部分，保证缓存有效）
    async fn build_turn_context(&self, user_input: &str) -> Result<TurnContext, HermesError> {
        // - 加载 Session 历史消息
        // - 从 Memory 预取相关记忆
        // - 注入启用的 Skill 内容
        // - 构建 tool schema
        // - 应用 prompt caching 标记
        todo!()
    }
}
```

### 7.2 上下文管理

```rust
/// 上下文管理器。负责：
/// - Token 计数（tiktoken-rs）
/// - 上下文窗口检查
/// - 上下文压缩触发
/// - Prompt Caching 标记管理
pub struct ContextManager {
    tokenizer: Arc<tiktoken_rs::CoreBPE>,
    max_context_tokens: u32,
    /// 标记为不可变的消息数量（用于 caching 的边界）
    cached_prefix_len: usize,
}

impl ContextManager {
    /// 检查消息总量是否超出上下文窗口
    pub fn would_exceed_context(&self, messages: &[Message], new_tokens: u32) -> bool {
        let current = self.count_tokens(messages);
        current + new_tokens > self.max_context_tokens
    }

    /// 应用 Anthropic Prompt Caching：
    /// 在不可变消息前缀的最后一组消息上添加 cache_control 标记
    pub fn apply_caching(&self, messages: &mut [Message]) {
        if self.cached_prefix_len > 0 && self.cached_prefix_len <= messages.len() {
            // 在前缀的最后一个 text block 上打标记
            let last = &mut messages[self.cached_prefix_len - 1];
            if let Some(ContentBlock::Text { .. }) = last.content.last_mut() {
                // 标记此 block 为 cache breakpoint
            }
        }
    }
}
```

### 7.3 错误分类与重试

```rust
/// API 错误分类器
pub struct ClassifiedError {
    pub original: HermesError,
    pub category: ErrorCategory,
    pub can_retry: bool,
    pub retry_after: Option<Duration>,
}

#[derive(Debug, PartialEq)]
pub enum ErrorCategory {
    /// 速率限制 → 指数退避重试
    RateLimited,
    /// 服务端临时错误 → 有限次数重试
    ServerError,
    /// 上下文超限 → 触发压缩后重试
    ContextTooLong,
    /// 认证错误 → 不重试，直接报错
    Authentication,
    /// 无效请求 → 不重试（除非工具调用格式错误可修复）
    InvalidRequest,
    /// 网络错误 → 有限次数重试
    Network,
}
```

---

## 八、Gateway 架构

### 8.1 GatewayRunner 设计

```rust
/// Gateway 主循环。同时管理：
/// - 多个消息平台的消息接收（StreamMap 合并）
/// - 多用户的 Agent 实例（LRU 缓存）
/// - 会话创建/恢复/过期管理
pub struct GatewayRunner {
    config: GatewayConfig,
    session_manager: Arc<SessionManager>,
    agent_cache: Arc<AgentCache>,
    tool_registry: Arc<ToolRegistry>,
    provider_factory: Arc<ProviderFactory>,
    /// 全局中断信号
    shutdown: watch::Receiver<bool>,
}

impl GatewayRunner {
    pub async fn run(&self) -> Result<(), HermesError> {
        // 1. 初始化启用的平台适配器
        let platforms = self.init_platforms().await?;

        // 2. 启动所有平台的消息接收
        let mut stream_map = StreamMap::new();
        for (name, adapter) in &platforms {
            let stream = adapter.receive_messages().await?;
            stream_map.insert(name.clone(), stream);
        }

        // 3. 主事件循环：select 消息流 / 关闭信号
        loop {
            tokio::select! {
                Some((platform_name, msg_result)) = stream_map.next() => {
                    match msg_result {
                        Ok(msg) => {
                            self.handle_message(&platform_name, msg).await;
                        }
                        Err(e) => {
                            tracing::error!(platform = %platform_name, error = %e,
                                "Message receive error");
                        }
                    }
                }
                _ = self.shutdown.changed() => {
                    tracing::info!("Gateway shutting down");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn handle_message(&self, platform: &str, msg: IncomingMessage) {
        let session_key = format!("{}:{}", platform, msg.chat_id);

        // 从 LRU 缓存获取或创建 Agent 实例
        let agent = self.agent_cache.get_or_create(&session_key, || {
            self.build_agent(platform, &msg)
        }).await;

        // 执行对话
        match agent.run_conversation(&msg.text).await {
            Ok(response) => {
                // 发送响应回原平台
                if let Some(adapter) = self.find_adapter(platform) {
                    let _ = adapter.send_message(&msg.chat_id, &response).await;
                }
            }
            Err(e) => {
                tracing::error!(session = %session_key, error = %e,
                    "Conversation error");
            }
        }
    }
}
```

### 8.2 Agent 实例 LRU 缓存

```rust
/// Agent 实例缓存（LRU 淘汰 + TTL 过期）。
///
/// 缓存 Agent 实例的目的：
/// 1. 保持 prompt cache 有效（同一会话复用同一个 Agent）
/// 2. 避免为每条消息重建 Agent（Provider 连接、Session 加载开销）
/// 3. TTL 机制防止内存无限增长
pub struct AgentCache {
    inner: Mutex<LruCache<String, CachedAgent>>,
    ttl: Duration,
    max_entries: usize,
}

struct CachedAgent {
    agent: Arc<Agent>,
    last_used: Instant,
    created_at: Instant,
}

impl AgentCache {
    pub async fn get_or_create<F>(
        &self,
        key: &str,
        factory: F,
    ) -> Arc<Agent>
    where
        F: FnOnce() -> AgentFuture,
    {
        let mut cache = self.inner.lock().unwrap();
        let now = Instant::now();

        if let Some(cached) = cache.get(key) {
            if now - cached.created_at < self.ttl {
                let agent = Arc::clone(&cached.agent);
                cache.get_mut(key).unwrap().last_used = now;
                return agent;
            }
            // TTL 过期，移除
            cache.pop(key);
        }

        // 创建新 Agent
        let agent = factory().await;
        let agent = Arc::new(agent);
        cache.put(key.to_string(), CachedAgent {
            agent: Arc::clone(&agent),
            last_used: now,
            created_at: now,
        });
        agent
    }
}
```

---

## 九、并发模型

### 9.1 线程模型

```
┌─────────────────────────────────────────────────────────────┐
│                 tokio Runtime (multi-thread)                  │
│                                                             │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │  CLI REPL    │  │   Gateway    │  │  Cron/Background  │  │
│  │  (单任务)    │  │   (多流并发) │  │  (spawn_blocking) │  │
│  └──────┬───────┘  └──────┬───────┘  └────────┬─────────┘  │
│         │                 │                    │            │
│         └─────────────────┼────────────────────┘            │
│                           ▼                                 │
│              ┌────────────────────────┐                    │
│              │   Agent Core (共享)     │                    │
│              │   Arc<Agent>           │                    │
│              │   ┌──────────────────┐ │                    │
│              │   │ RwLock<State>    │ │  ← 多读单写         │
│              │   └──────────────────┘ │                    │
│              │   ┌──────────────────┐ │                    │
│              │   │ Provider (Arc)   │ │  ← 线程安全的HTTP客户端 │
│              │   └──────────────────┘ │                    │
│              └────────────────────────┘                    │
└─────────────────────────────────────────────────────────────┘
```

### 9.2 各场景并发策略

| 场景 | 策略 | 关键点 |
|------|------|--------|
| 多用户 Gateway | 每用户独立 `Arc<Agent>` | 用户间完全隔离，RwLock 保证单 Agent 内安全 |
| 并行工具执行 | `futures::join_all` | 多个工具同时执行，各自超时独立 |
| Session 读写 | SQLite WAL 模式 | 读不阻塞写，写不阻塞读 |
| Prompt Caching 读 | `RwLock::read()` | 多线程可并发读消息历史 |
| API Key 轮转 | `Mutex<VecDeque<Credential>>` | 轮转池线程安全 |
| Ctrl+C 中断 | `Arc<AtomicBool>` | 无锁中断标志 |
| Cron 定时任务 | `tokio::spawn` 独立任务 | 不阻塞主循环 |
| 后台 Memory sync | `tokio::spawn` fire-and-forget | 不阻塞用户响应 |

---

## 十、配置系统

### 10.1 配置层级（对齐 Python 版）

```
优先级（从低到高）：
  1. 代码默认值
  2. ~/.hermes/config.yaml  （全局配置）
  3. Profile 配置           （独立岛屿，不继承）
  4. 环境变量               （仅用于密钥/Token）
```

### 10.2 数据结构

```rust
/// 配置结构（对应 config.yaml）
///
/// 使用 `config` crate 加载，支持：
/// - 默认值（代码内）
/// - YAML 文件
/// - 环境变量覆盖（前缀 HERMES_）
#[derive(Debug, Deserialize, Clone)]
pub struct HermesConfig {
    #[serde(default = "default_model")]
    pub default_model: String,

    #[serde(default = "default_toolsets")]
    pub default_toolsets: Vec<String>,

    #[serde(default)]
    pub temperature: Option<f32>,

    #[serde(default)]
    pub max_tool_iterations: Option<usize>,

    /// Profile 配置（独立岛屿，互不继承）
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,

    /// Provider 认证配置
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,

    /// Gateway 配置（可选）
    pub gateway: Option<GatewayConfig>,

    /// 日志级别
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

/// Profile 配置
///
/// Profile 是独立的配置岛屿：
/// - 不继承其他 Profile
/// - 创建时可通过 --clone 从现有 Profile 复制
/// - 切换 Profile 即切换整个运行环境（模型、工具集、温度等）
#[derive(Debug, Deserialize, Clone)]
pub struct ProfileConfig {
    pub model: Option<String>,
    pub toolsets: Option<Vec<String>>,
    pub temperature: Option<f32>,
    pub max_tool_iterations: Option<usize>,
    pub thinking: Option<bool>,
    pub system_prompt: Option<String>,
}

/// Provider 认证配置
#[derive(Debug, Deserialize, Clone)]
pub struct ProviderConfig {
    /// "openai" | "anthropic" | "gemini" | "bedrock" | "ollama"
    pub provider_type: String,

    /// 自定义 base URL（如 Azure OpenAI / 本地代理）
    pub base_url: Option<String>,

    /// API key 来源的环境变量名（如 "OPENAI_API_KEY"）
    pub api_key_env: String,

    /// 额外的 API key 环境变量（用于 key 池）
    #[serde(default)]
    pub extra_api_key_envs: Vec<String>,

    /// HTTP 超时（秒）
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}
```

---

## 十一、安全设计

### 11.1 工具安全沙箱

```rust
/// 安全执行上下文。所有工具执行前经过此层校验。
pub struct SecurityContext {
    /// 允许的工作目录（工具不能操作目录外的文件）
    allowed_dirs: Vec<PathBuf>,
    /// 命令白名单（shell 工具的限制）
    allowed_commands: Option<Vec<String>>,
    /// 禁止的命令
    blocked_commands: Vec<String>,
    /// 文件大小上限（防止 OOM/DOS）
    max_file_size: u64,
    /// 允许的文件扩展名
    allowed_extensions: Option<Vec<String>>,
}

impl SecurityContext {
    /// 校验文件路径是否安全（防止路径遍历 ../ 攻击）
    pub fn validate_path(&self, path: &Path) -> Result<PathBuf, HermesError> {
        let canonical = path.canonicalize()
            .map_err(|_| HermesError::security("Invalid path"))?;
        let allowed = self.allowed_dirs.iter().any(|d| canonical.starts_with(d));
        if !allowed {
            return Err(HermesError::security(
                format!("Path outside allowed directories: {}", canonical.display())
            ));
        }
        Ok(canonical)
    }

    /// 校验 shell 命令是否安全
    pub fn validate_command(&self, cmd: &str) -> Result<(), HermesError> {
        if let Some(whitelist) = &self.allowed_commands {
            let cmd_name = cmd.split_whitespace().next().unwrap_or("");
            if !whitelist.iter().any(|w| w == cmd_name) {
                return Err(HermesError::security(
                    format!("Command not in whitelist: {}", cmd_name)
                ));
            }
        }
        if self.blocked_commands.iter().any(|b| cmd.starts_with(b)) {
            return Err(HermesError::security(
                format!("Blocked command detected: {}", cmd)
            ));
        }
        Ok(())
    }
}
```

### 11.2 敏感信息防护

| 层面 | 措施 |
|------|------|
| 环境变量 | API Key 仅通过环境变量传入，不出现在 config.yaml |
| 日志脱敏 | `tracing` 的 `Value` 实现自动截断 API key（只显示首尾各 4 字符） |
| 内存 | 凭证使用 `secrecy::SecretString`，Drop 时清零 |
| Session 存储 | SQLite 文件权限 0600，数据库内容不包含原始 key |
| 网络 | 所有 Provider 请求强制 HTTPS |

---

## 十二、实施路线图

### 12.1 里程碑

| 里程碑 | 内容 | 时间 | 交付物 |
|--------|------|------|--------|
| **M1: MVP** | 核心对话 + OpenAI/Anthropic + terminal/file/web 工具 + CLI REPL | 12-16 周 | 可运行的 CLI 工具 |
| **M2: 生产就绪** | + Session SQLite + Memory + Skill 系统 + Prompt Caching | 20-26 周 | 可部署的二进制 |
| **M3: Gateway** | + 多平台消息网关 + Agent 缓存 + 会话管理 | 28-32 周 | Gateway 可用 |
| **M4: 完整功能** | + TUI + 插件系统 + Browser 工具 + 剩余 Provider | 32-36 周 | 功能完整的 Agent 平台 |

### 12.2 详细阶段

#### 阶段 1: 基础骨架 (1-2 周)

- [ ] 创建 Cargo Workspace + 5 个 crate
- [ ] CI/CD (GitHub Actions: test + lint + clippy)
- [ ] `rustfmt.toml` + `clippy.toml`
- [ ] `hermes-common`: 核心类型定义 (`Message`, `Role`, `ContentBlock`, `TokenUsage`)
- [ ] `hermes-common`: `HermesError` (thiserror)
- [ ] `hermes-common`: `HermesConfig` / `ProfileConfig` (serde)
- [ ] 日志系统 (tracing + tracing-subscriber)

#### 阶段 2: Provider 适配 (3-4 周)

- [ ] `LLMProvider` trait 定义
- [ ] `ProviderCapabilities` 标记
- [ ] `OpenAIProvider` 实现（chat + stream + SSE 解析）
- [ ] `AnthropicProvider` 实现（chat + stream + thinking + cache_control）
- [ ] `GeminiProvider` 实现（格式互转）
- [ ] `ProviderFactory` + 模型路由
- [ ] `CredentialPool` 多 key 轮转
- [ ] 集成测试（wiremock）

#### 阶段 3: 工具系统 (3 周)

- [ ] `Tool` trait + `ToolRegistry` + `ToolSchema`
- [ ] `#[register_tool]` 宏（编译期注册）
- [ ] 工具调度器（并发执行 + 超时 + 重试）
- [ ] `TerminalTool`（含安全沙箱）
- [ ] `FileTools`（read/write/patch/search）
- [ ] `WebTools`（web_search + web_extract）
- [ ] `SecurityContext` 路径/命令校验
- [ ] 单元测试 + 安全测试

#### 阶段 4: Agent 核心 (3 周)

- [ ] `AgentConfig` + `Agent` 结构体
- [ ] `ConversationState` (RwLock 保护)
- [ ] `IterationBudget` 预算控制
- [ ] 对话循环 `run_conversation()`
- [ ] 上下文管理 + Token 计数 (tiktoken-rs)
- [ ] Prompt Caching 优化
- [ ] 错误分类 + 指数退避重试
- [ ] 工具调用 JSON 修复
- [ ] `ContextCompressor` 上下文压缩
- [ ] 流式响应 + Ctrl+C 中断

#### 阶段 5: 会话与记忆 (2 周)

- [ ] `SessionStore` trait
- [ ] `SqliteSessionStore`（SQLx + FTS5 + WAL）
- [ ] `MemoryProvider` trait
- [ ] `BuiltinMemoryProvider` (SQLite)
- [ ] `MemoryManager`（多 Provider 管理）
- [ ] 集成测试（testcontainers）

#### 阶段 6: Skill 系统 (2 周)

- [ ] Skill 结构定义
- [ ] Skill 加载器（文件系统 / URL）
- [ ] 模板变量替换 (`${HERMES_SKILL_DIR}` 等)
- [ ] Inline shell 执行
- [ ] `SkillManager`（启用/禁用/搜索/缓存）

#### 阶段 7: CLI (2 周)

- [ ] `clap` CLI 命令结构
- [ ] 配置文件加载 (config crate)
- [ ] 多 Profile 支持
- [ ] 交互式 REPL (rustyline)
- [ ] 彩色输出 (colored)
- [ ] Markdown 渲染 (termimad)

#### 阶段 8: Gateway (3 周，后续)

- [ ] `PlatformAdapter` trait
- [ ] `GatewayRunner` 主循环
- [ ] `SessionManager` 多用户会话
- [ ] `AgentCache` LRU 缓存
- [ ] Telegram 适配器 (teloxide)
- [ ] Discord 适配器 (serenity)
- [ ] 其余平台适配器（按需）

#### 阶段 9: 插件系统 (2 周，后续)

- [ ] `Plugin` trait 定义
- [ ] 动态库加载 (libloading)
- [ ] 插件发现与注册
- [ ] WASM 插件支持（可选，wasmer）

#### 阶段 10: 测试、优化与文档 (3 周)

- [ ] 单元测试（每个模块 >80% 覆盖）
- [ ] 集成测试（testcontainers + wiremock）
- [ ] E2E 测试（真实 LLM API）
- [ ] 基准测试（criterion）
- [ ] 性能优化（内存、延迟）
- [ ] `cargo doc` API 文档
- [ ] `mdbook` 用户手册

---

## 十三、技术选型与决策记录

### 13.1 技术选型

| 组件 | 选型 | 理由 |
|------|------|------|
| 异步运行时 | `tokio` (full features) | Rust 异步生态事实标准 |
| HTTP 客户端 | `reqwest` (rustls-tls) | 功能完善，原生支持 SSE 流 |
| 数据库 | `sqlx` (SQLite + PostgreSQL) | 编译时 SQL 检查，原生异步 |
| 序列化 | `serde` + `serde_json` | 生态标准 |
| 错误处理 | `thiserror` + `anyhow` | 库用 thiserror，应用用 anyhow |
| 日志追踪 | `tracing` + `tracing-subscriber` | 结构化日志，异步友好 |
| CLI 解析 | `clap` (derive mode) | 声明式，功能强大 |
| TUI | `ratatui` + `crossterm` | 跨平台，活跃维护 |
| REPL | `rustyline` | 类似 readline，支持历史/补全 |
| Token 计数 | `tiktoken-rs` | OpenAI 官方 Rust 绑定 |
| 配置 | `config` crate | 多源配置（文件+环境变量） |
| 编译期注册 | `linkme` | 零成本分布式切片 |
| 凭证安全 | `secrecy` | 内存安全（Drop 时清零） |
| 测试 HTTP Mock | `wiremock` | 异步 HTTP mock 服务 |
| 测试数据库 | `testcontainers` | 真实数据库集成测试 |
| 基准测试 | `criterion` | 统计化基准测试框架 |

### 13.2 关键决策记录

| # | 决策 | 理由 |
|---|------|------|
| 1 | **LLM Client: 手写 HTTP，不用社区 SDK** | 保持对 Provider API 变更的完全控制，避免社区库滞后/被弃用 |
| 2 | **工具注册: 编译期 linkme，非运行时扫描** | 零运行时开销，链接时错误检查，避免 Python 动态 import 的启动延迟 |
| 3 | **Session Store: trait 抽象，默认 SQLite** | SQLite 零配置覆盖单机部署，PostgreSQL trait 实现覆盖集群部署 |
| 4 | **并发: Arc<RwLock<T>> 而非 Actor 模型** | 读多写少场景 RwLock 更优，Actor 增加复杂度且无性能优势 |
| 5 | **Prompt Caching: Agent 内不可变前缀** | 保证 long-living conversation 的缓存持续有效 |
| 6 | **Skill 模板: 自实现，不引入 Jinja2 等价物** | Skill 模板仅需简单变量替换+inline shell，完整模板引擎过度 |
| 7 | **单 Workspace** | crate 间强耦合，共享 Cargo.lock 保证版本一致，增量编译快 |
| 8 | **配置文件: .env 仅用于密钥** | 对齐 Python 版规则：行为配置在 config.yaml，密钥在环境变量 |
| 9 | **插件隔离: 短期 libloading，长期 WASM** | libloading 快速实现 MVP，WASM 提供更强的安全隔离 |
| 10 | **crate 拆分 5 个而非 10+** | 避免过度拆分导致的编译复杂度和依赖管理负担 |

### 13.3 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| Rust 异步编程复杂 | 死锁、编译错误 | 避免嵌套 RwLock 写锁，`tokio::select!` 处理多路选择 |
| Provider API 频繁变更 | 维护成本高 | 独立 Adapter 层，隔离内部标准化模型与外部 API |
| 工具安全漏洞 | RCE | `SecurityContext` 校验层 + 命令白名单 + 路径遍历检测 |
| tiktoken-rs 跨平台编译 | macOS/Windows 构建失败 | CI 矩阵测试，提供预编译二进制或 fallback 方案 |
| 大型 Skill 集加载慢 | CLI 启动延迟 | 懒加载 + 内存缓存 + 后台预加载 |

---

## 14. 团队协作建议

**2-3 人团队分工**：
- **人员 A (Core)**: hermes-common + hermes-core（Provider + Tool + Agent 循环）
- **人员 B (Storage)**: Session + Memory + Skill 系统
- **人员 C (Edge)**: CLI / TUI / Gateway / Plugin

**独立开发者**：
- 严格按 MVP → 生产就绪 → 完整功能 的顺序迭代
- 每周设小目标，`cargo watch -x check` 实时验证
- MVP 阶段使用 mock Provider 替代真实 API 调用加速开发验证
