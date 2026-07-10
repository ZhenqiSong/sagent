# CODEBUDDY.md This file provides guidance to CodeBuddy when working with code in this repository.

## 项目概述

sagent 是一个个人智能 Agent 项目，采用 Rust 实现，目标为单二进制、低资源占用、生产级部署。目前处于**阶段 1（基础骨架）**的早期，仅完成 Cargo 项目初始化。

详细架构设计文档：[docs/rust-impl-plan.md](docs/rust-impl-plan.md)

## 常用命令

```bash
# 构建项目
cargo build

# 构建（release 模式）
cargo build --release

# 运行
cargo run

# 运行测试
cargo test

# 运行单个测试
cargo test <test_name>

# 代码检查（需要安装 clippy）
cargo clippy -- -D warnings

# 代码格式化（需要安装 rustfmt）
cargo fmt --check

# 持续检查（开发时实时编译验证）
cargo watch -x check

# 生成文档
cargo doc --open

# 运行基准测试（后续阶段添加 criterion 后使用）
cargo bench
```

## 核心架构

### Crate 依赖层级（5 个 crate，单 Cargo Workspace）

```
sagent-cli ─────────────┐
                        │
sagent-gateway ─────────┼──► sagent-core ──► sagent-common
                        │
sagent-plugin ◄─────────┘  (SDK，仅定义 trait，不依赖 core)
```

- **`sagent-common`**：零 IO 依赖，纯数据结构（Message、Role、ContentBlock）和工具函数，所有 crate 可安全依赖。使用 `thiserror` 定义 `SagentError`，使用 `serde` 进行配置序列化。
- **`sagent-core`**：主逻辑层，包含 Agent 对话循环、Provider 适配、Tool 系统、Session/Memory/Skill 管理。不依赖 CLI/Gateway。
- **`sagent-cli`**：二进制入口（`clap` 命令解析），提供交互式 REPL（`rustyline`）和 TUI（`ratatui`）。
- **`sagent-gateway`**：多平台消息网关，同时管理多个消息平台（Telegram/Discord/Slack 等）的消息流，通过 LRU 缓存管理多用户 Agent 实例。
- **`sagent-plugin`**：轻量 SDK，仅定义 `Plugin` trait 和 `PluginHook`，供第三方插件 crate 依赖。短期使用 `libloading` 加载动态库，长期考虑 WASM 隔离。

选择单一 Workspace 的原因：crate 间紧密耦合、共享 `Cargo.lock` 保证依赖版本一致、共享 `target/` 目录增量编译快、统一版本号发布。

### 核心 Trait 体系（6 个关键抽象）

1. **`LLMProvider`**：模型提供商统一接口。每个 Provider（OpenAI/Anthropic/Gemini/Bedrock/Ollama）实现此 trait，负责内部格式转换（如 tool schema 的 OpenAI ↔ Anthropic 互转）和 system prompt 的位置适配。使用手写 HTTP 客户端（`reqwest`）而非社区 SDK，以保持对 API 变更的完全控制。

2. **`Tool`**：工具抽象。通过 `#[register_tool]` 宏 + `linkme::distributed_slice` 实现**编译期自动注册**到全局 `BUILTIN_TOOLS` 切片，无需运行时扫描。关键方法：
   - `schema() -> ToolSchema`：定义发送给 LLM 的函数描述
   - `check_availability() -> bool`：门控检查，返回 false 的工具不出现在 LLM schema 中（节省 token，避免调用失败）
   - `execute(args, ctx) -> ToolResult`：在 tokio 任务中并发执行

3. **`SessionStore`**：会话持久化。默认 SQLite + FTS5 全文搜索（WAL 模式支持读写并发），可选 PostgreSQL + pgvector。每个 Profile 拥有独立 Store 实例，Session 之间完全隔离。

4. **`MemoryProvider`**：长期记忆。支持多个 Provider 协同（本地 SQLite + 云端 API），对话结束后异步 `sync()` 不阻塞用户响应。

5. **`PlatformAdapter`**：消息平台接入。每个平台实现此 trait，Gateway 通过 `StreamMap` 合并多个平台的消息流，在 `tokio::select!` 中统一处理。

6. **`Plugin`**：插件接口。第三方通过动态库提供自定义工具、生命周期 Hook 和自定义 Provider。

### Agent 核心数据流

Agent 的 `run_conversation()` 是整个系统最核心的路径：

1. **构建 turn 上下文**（循环外，保证 prompt caching 不可变前缀）：加载 Session 消息历史 → 获取 Memory prefetch → 注入启用的 Skill 内容 → 构建 system prompt → 构建 tool schema（仅包含 check_availability()=true 的工具）
2. **对话循环**（最多 N 轮，受 `IterationBudget` 控制）：
   - 调用 `provider.chat()` 发送请求
   - 响应分叉：`text` → 结束循环返回给用户；`tool_calls` → 通过 `ToolRegistry::dispatch()` 并发执行工具（`join_all` + 各自超时 + 结果截断防 token 爆炸）→ 追加 tool_result 继续循环
   - 错误分类处理：`RateLimited` → 指数退避重试；`ContextTooLong` → 触发 `ContextCompressor` 压缩后重试；`Authentication`/`InvalidRequest` → 不重试
3. **Post-turn 后处理**：追加消息到 Session Store → 触发 Memory sync（后台 fire-and-forget）→ 更新 Token 统计

### 并发模型

- 异步运行时：`tokio`（multi-thread）
- Agent 实例：`Agent` 由 `Arc` 包裹共享，可变状态 `ConversationState` 用 `RwLock<T>` 保护（读多写少场景）
- Gateway 多用户：每个用户独立 `Arc<Agent>` 实例，用户间完全隔离
- 工具执行：`futures::join_all` 并发执行，各自独立超时
- 中断信号：`Arc<AtomicBool>` 无锁标志，支持 Ctrl+C
- 后台任务（Memory sync、Cron）：`tokio::spawn` 独立任务，不阻塞主循环

### 关键设计决策

- **Prompt Caching 不可触碰**：system prompt + tool schema 在对话循环外构建后不可变，保证长对话中缓存持续有效
- **Profile 是独立岛屿**：每个 Profile 拥有独立 `Config` 实例，互不继承，切换 Profile 即切换整个运行环境
- **门控工具**：`check_availability()` 返回 false 的工具完全不出现在 LLM 视野中，节省 token 并避免调用失败
- **配置层级**：代码默认值 → `~/.sagent/config.yaml`（全局）→ Profile 配置（独立）→ 环境变量（仅用于密钥/Token）。行为配置在 config.yaml，密钥在环境变量
- **LLM Client 手写 HTTP**：不依赖社区 SDK，保持对 Provider API 变更的完全控制
- **上下文压缩采用写时复制**：压缩时创建新消息列表，不修改原始缓存前缀
- **安全设计**：`SecurityContext` 校验层实现路径遍历检测、命令白名单/黑名单、文件大小限制；凭证使用 `secrecy::SecretString`（Drop 清零）；日志自动脱敏 API key

### 技术选型速查

| 组件 | 选型 |
|------|------|
| 异步运行时 | `tokio` (full features) |
| HTTP 客户端 | `reqwest` (rustls-tls) |
| 数据库 | `sqlx` (SQLite + PostgreSQL) |
| 序列化 | `serde` + `serde_json` |
| 错误处理 | `thiserror` (库) + `anyhow` (应用) |
| 日志 | `tracing` + `tracing-subscriber` |
| CLI | `clap` (derive mode) |
| REPL | `rustyline` |
| TUI | `ratatui` + `crossterm` |
| Token 计数 | `tiktoken-rs` |
| 配置 | `config` crate |
| 编译期注册 | `linkme` |
| 凭证安全 | `secrecy` |
| 测试 Mock | `wiremock` + `testcontainers` |

### 当前实施状态

项目处于阶段 1（基础骨架）的早期阶段，当前 `src/main.rs` 仅为 `Hello, world!`。下一步应按实施路线图执行：

- **阶段 1**：创建 Cargo Workspace + 5 个 crate、CI/CD、核心类型定义、日志系统
- **阶段 2**：`LLMProvider` trait + OpenAI/Anthropic/Gemini 适配器实现
- **阶段 3**：`Tool` trait + `ToolRegistry` + `#[register_tool]` 宏 + 基础工具（terminal/file/web）
- **阶段 4**：Agent 核心循环 + 上下文管理 + Prompt Caching + 错误重试
- **阶段 5**：Session SQLite FTS5 + Memory 系统
- **阶段 6**：Skill 加载/模板/管理
- **阶段 7**：CLI REPL + TUI
- **阶段 8+**：Gateway、插件系统、测试与优化

详细实施计划见 `docs/rust-impl-plan.md` 第十二章。
