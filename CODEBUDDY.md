# CODEBUDDY.md This file provides guidance to CodeBuddy when working with code in this repository.

## 项目概述

sagent 是 Hermes Agent（Python 项目，~12 万行）的全量 Rust 重写，目标为单二进制、低资源占用、生产级部署。全新独立项目，不兼容 Python 版的配置/数据库格式。

完整重写计划书：[plans/sagent-rust-rewrite-plan.md](plans/sagent-rust-rewrite-plan.md)
阶段 0 详细实施计划：[plans/phase0-foundation.md](plans/phase0-foundation.md)

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
cargo build -p sagent-common

# 代码检查（clippy pedantic，零警告要求）
cargo clippy -- -D warnings

# 代码格式化检查
cargo fmt --check

# 依赖审计
cargo audit

# 运行集成测试
cargo test --test integration -- config_roundtrip

# 生成文档
cargo doc --open
```

## 核心架构

### Cargo Workspace 结构（10 个 crate）

按照重写计划书，crate 按依赖顺序自底向上排列：

```
sagent-cli ────────────────┐
sagent-gateway ────────────┤
sagent-browser ────────────┼──► sagent-core ──► sagent-store ──► sagent-proto ──► sagent-common
sagent-mcp ────────────────┤
sagent-plugins ────────────┘
sagent-config ─────────────┘  (不依赖 core，独立配置/路径管理)
```

- **`sagent-common`**：零 IO 依赖，纯基础能力。`SagentError`（`thiserror`，含 `FailoverReason` 23 个枚举值 + `ClassifiedError` 分类）、`tracing` 日志（agent/errors/gateway 三个文件 + `RedactingLayer` 敏感信息脱敏）、`Redact` trait（JWT/Slack token/API key/phone/email/UUID 正则遮蔽）、`TokenBucket` 速率限制、`I18nKey` 占位。
- **`sagent-proto`**：共享数据类型（serde），无 IO。`Role`/`ContentPart`/`Message`（OpenAI 兼容格式）、`ToolCall`/`ToolResult`/`ToolDefinition`、`Platform`（`KnownPlatform` 24 成员 + 动态扩展）、`SessionSource`（20+ 字段）、`SessionKey`、`ChatType`、`MessageType`（10 变体）、`MessageEvent`、`SendResult`、`Usage`/`NormalizedResponse`/`FinishReason`。
- **`sagent-config`**：配置与路径。`get_sagent_home()` 三级优先级（`SAGENT_HOME` env → Windows `%LOCALAPPDATA%\sagent` → POSIX `~/.sagent`）、profile 管理（独立岛屿，无默认继承，`--clone` 复制）、`SagentConfig` 从 `config.yaml` 加载（serde_yaml）、`.env` 仅读密钥。行为配置在 config.yaml，密钥在环境变量——这是硬性规则。
- **`sagent-store`**：SQLite 持久化层（`rusqlite`）。会话 DB（消息存储 + FTS5 全文检索 + 会话恢复/导出 md/html）、memory DB、projects DB、kanban DB。进程级单例锁防多进程写冲突。secrets 加密用 AES-GCM + Argon2 KDF。
- **`sagent-core`**：Agent 内核。对话循环（`conversation_loop.rs`）、Provider 适配（OpenAI/Anthropic/Gemini/Bedrock/Vertex/Azure/DeepSeek/Ollama，统一 `LlmProvider` trait）、Prompt Caching、上下文压缩（写时复制，保留消息交替不变量）、Tool dispatch（危险命令审批 guardrail）、消息卫生（role 交替校验）、子代理 delegation/subsessions、计费/用量追踪。
- **`sagent-tools`**：工具系统。注册表（`toolsets.rs`）+ 内置工具（terminal/file/web/browser）+ 运行时环境（local/docker/ssh/modal/daytona，基于 `portable-pty`）+ Skills 管理 + Memory 工具 + Agent 内建工具（delegate/todo/clarify/approval/cronjob 等，service-gated 按后端配置暴露）+ 安全层（路径安全、威胁模式、URL 安全）。
- **`sagent-cli`**：二进制入口（`clap` 44 子命令树）+ 交互式 REPL（`dialoguer`）+ TUI（`ratatui` + `crossterm`）+ Setup 向导。对应 Python 版 `cli.py` + `hermes_cli/`。
- **`sagent-gateway`**：多平台消息网关核心 + 20+ 平台适配器（Telegram/Discord/Slack/WhatsApp/Signal/Matrix/LINE/WeChat/QQBot 等），统一 `BasePlatformAdapter` trait。消息入站 dispatch、授权映射、交互回调约定（`cl:`/`appr:`/`sc:` 按钮 id 前缀）、cron 投递、webhook receiver（`axum`）。
- **`sagent-mcp`**：MCP 客户端（stdio/SSE transport）+ OAuth 流。MCP catalog 工具与内置工具走同一 dispatch 路径。
- **`sagent-browser`**：CDP 浏览器驱动（`tokio-tungstenite` + CDP 协议），提供 browser/supervisor 工具。
- **`sagent-plugins`**：插件运行时。推荐 WASM via extism（沙箱化、跨语言），辅以 `libloading` 动态库（受信第一方）。类别：platform adapter、skill provider、memory backend、notifier、tool provider。

### 核心设计锚点（贯穿所有阶段）

1. **Prompt caching 是神圣的**：system prompt 在对话生命周期内 byte-stable；tools schema 不随轮次变化；上下文压缩仅在显式触发时发生，压缩后重建缓存前缀。阶段 2 即建立缓存前缀哈希测试作为回归护栏。
2. **窄腰原则**：新增模型工具的优先级——扩展现有代码 → CLI 命令 + skill → service-gated tool（`check_fn`）→ 插件 → MCP catalog → 新 core tool（最后手段）。
3. **交替不变量**：消息序列绝不允许连续两条同角色消息，绝不在循环中注入合成 user 消息。`message_sanitization.rs` 负责校验。
4. **行为契约测试 > 快照测试**：断言不变量（交替、缓存前缀哈希、路径拒绝），不 freeze 枚举计数/版本字面量（避免 change-detector 测试）。

### Agent 核心数据流

Agent 的对话循环（`conversation_loop.rs`）是系统最核心路径：

1. **构建 turn 上下文**（循环外，保证 prompt caching 不可变前缀）：加载 Session 消息历史 → Memory prefetch → 注入 Skill 内容 → 构建 system prompt（`minijinja` 模板 + `PLATFORM_HINTS`）→ 构建 tool schema（仅包含 service-gated 检查通过的工具）
2. **对话循环**（最多 N 轮，`IterationBudget` 控制）：
   - 调用 `provider.chat()` 发送请求（手写 HTTP `reqwest`，不依赖社区 SDK）
   - 响应分叉：`text` → 结束循环返回用户；`tool_calls` → `ToolRegistry::dispatch()` 并发执行（`join_all` + 各自超时 + 结果截断防 token 爆炸）→ 追加 tool_result 继续循环
   - 错误分类处理：`RateLimited` → 指数退避重试；`ContextTooLong` → 触发 `ContextCompressor` 压缩后重试；`Authentication`/`InvalidRequest` → 不重试
3. **Post-turn**：追加消息到 Session Store → Memory sync（后台 fire-and-forget）→ 更新 Token 统计

### 并发模型

- 异步运行时：`tokio`（multi-thread）
- Agent 实例：`Arc` 包裹共享，可变状态 `ConversationState` 用 `RwLock<T>` 保护（读多写少）
- Gateway 多用户：每个用户独立 `Arc<Agent>` 实例，用户间完全隔离
- 工具执行：`futures::join_all` 并发执行，各自独立超时
- 中断信号：`Arc<AtomicBool>` 无锁标志，支持 Ctrl+C
- 后台任务（Memory sync、Cron）：`tokio::spawn` 独立任务，不阻塞主循环

### 安全设计

- **`SecurityContext`**：路径遍历检测、命令白名单/黑名单、文件大小限制
- **凭证安全**：`secrecy::SecretString`（Drop 清零）；日志 `RedactingLayer` 自动脱敏 API key/token/phone/email
- **secrets 加密**：AES-GCM + Argon2 KDF，密钥从 `.env`/keychain 派生
- **危险命令审批**：`tool_guardrails.rs` 默认拦截 `rm -rf /` 等操作，需用户确认
- **供应链安全**：`cargo-deny` + 锁定 `Cargo.lock`；依赖最小化；secrets 不进依赖

### 技术选型速查

| 组件 | 选型 |
|------|------|
| 异步运行时 | `tokio` (full features) |
| HTTP 客户端 | `reqwest` (rustls-tls, socks proxy) |
| 数据库 | `rusqlite` (FTS5) |
| 序列化 | `serde` + `serde_yaml` + `serde_json` + `serde_with` |
| 错误处理 | `thiserror` (库) + `anyhow` (应用) |
| 日志 | `tracing` + `tracing-subscriber` + `tracing-appender` |
| CLI | `clap` (derive mode) |
| TUI/交互 | `ratatui` + `crossterm` + `dialoguer` |
| HTTP 服务 | `axum` |
| PTY 终端 | `portable-pty` |
| Prompt 模板 | `minijinja` |
| 配置 | `serde_yaml` + `dirs` |
| 环境变量 | `dotenvy` |
| 凭证安全 | `secrecy` |
| 加密 | `rustcrypto` (aes-gcm, sha2, argon2) |
| 进程监控 | `sysinfo` |
| WebSocket | `tokio-tungstenite` |
| Markdown | `comrak` |
| gitignore | `ignore` crate |
| 测试 Mock | `wiremock` |
| 依赖审计 | `cargo-deny` + `cargo-audit` |

### 分阶段路线

项目采用自底向上、逐阶段独立编译测试的策略：

| 阶段 | 内容 | 优先级 |
|------|------|--------|
| **0 奠基** | Workspace 骨架、`sagent-common`/`sagent-proto`/`sagent-config`/`sagent-cli` 最小可用、CI/CD 跨平台矩阵 | P0 |
| **1 配置/持久化** | Profile 体系、SQLite 存储（会话 FTS5/memory/projects/kanban）、secrets 加密、进程锁 | P0 |
| **2 Agent 内核** | 对话循环、Provider 适配（8+ backend）、Prompt Caching、上下文压缩、消息卫生、子代理、计费 | P0 |
| **3 工具/环境** | 注册表、terminal/file/web/browser 工具、多环境后端、MCP 客户端、Skills、Memory、安全层 | P0 |
| **4 CLI/TUI** | 44 子命令树、交互式 REPL、ratatui TUI、Setup 向导 | P1 |
| **5 Gateway** | 核心 + 20+ 平台适配器、webhook receiver、cron 投递、交互回调 | P1 |
| **6 插件** | WASM (extism) / 动态库 (libloading) 插件运行时 | P2 |
| **7 桌面端** | Electron 前端复用 + Rust 后端 IPC/HTTP 桥接 | P2 |
| **8 测试/发布** | E2E、不变量测试套件、性能基线、单二进制 `cargo-dist` 发布 | P1 |

**MVP 里程碑**（阶段 0-3 + 单个 gateway 平台）：终端跑 agent、执行 terminal/file/web/browser 工具、接 Telegram 收发消息。

### 实施策略

每个阶段采用**自底向上依赖顺序**，每个步骤只实现一个最小可编译可验证的功能单元，完成即 `cargo build` 确认零错误零警告，再进入下一个。绝不一次写完整个 crate 再编译。

测试策略：行为契约 > 快照——用 `#[test]` 断言不变量（交替、缓存前缀哈希、路径拒绝），不 freeze 枚举计数/版本字面量。E2E 使用临时 `SAGENT_HOME` + 真实 SQLite + wiremock。

### 关键设计决策

- **Platform 枚举支持动态扩展**：内部 `KnownPlatform` 24 成员 + `Platform(String)` 包装类型（`#[serde(transparent)]`），已知平台零开销转换，未知平台（插件动态注册）保留原始字符串。
- **LLM Client 手写 HTTP**：不依赖社区 SDK（如 `openai` crate），保持对 Provider API 变更的完全控制。
- **Prompt Caching 不可触碰**：system prompt + tool schema 在对话循环外构建后不可变，压缩采用写时复制（新消息列表，不修改原始缓存前缀）。
- **Profile 是独立岛屿**：每个 Profile 拥有独立 `Config` 实例，互不继承，切换 Profile 即切换整个运行环境。仅提供 `--clone` 复制。
- **门控工具（service-gated）**：`check_availability()` 返回 false 的工具完全不出现在 LLM 视野中，节省 token 并避免调用失败。
- **配置层级**：代码默认值 → `config.yaml`（全局）→ Profile 配置（独立）→ `.env`（仅密钥）。行为配置在 config.yaml，密钥在 `.env`。
- **Electron 前端复用**：桌面端保留 TypeScript/React 前端，仅将后端从 Python 进程替换为 Rust 进程，通过 stdio IPC / 本地 HTTP（`axum`）通信。
- **消息角色交替保证**：消息序列经 `message_sanitization.rs` 校验后绝不出现连续同角色，绝不在循环中注入合成 user 消息。
- **交互回调约定跨平台共享**：按钮 id 约定 `cl:<id>:<idx>`（clarify）、`appr:<id>:<choice>`（approval）、`sc:<choice>:<id>`（slash confirm），gateway resolver 通用。
- **用户可见错误信息使用中文**：所有 `thiserror` 的 `#[error("...")]` 消息、`Display` 实现、以及通过日志/tui/cli 最终暴露给用户的错误文本，一律使用中文。内部开发日志（`tracing::debug!`/`trace!`）和技术性错误上下文可使用英文。
- **日志详细且分级明确**：使用 `tracing` 的 5 级日志体系。`ERROR`：需要立即关注的故障（Provider 认证失败、数据库损坏、密钥解密失败），必须包含足够上下文（操作名称、关键参数、原始错误链）。`WARN`：可恢复的异常（重试、降级、速率限制触发、压缩执行），记录触发原因和恢复策略。`INFO`：关键业务流程节点（会话开始/结束、工具调用、Provider 切换、配置加载），形成可审计的操作轨迹。`DEBUG`：详细的请求/响应数据（脱敏后）、消息内容摘要、缓存命中/未命中。`TRACE`：完整原始数据、函数进入/退出。每条日志必须携带结构化字段（`session_id`、`provider`、`tool_name` 等 span 属性），便于按维度过滤排查。

### 当前实施状态

项目处于**阶段 0（工程奠基）**的开始阶段。`Cargo.toml` 已配置 workspace（`members = ["crates/*"]`）但 crates 目录尚未创建。下一步应按 [plans/phase0-foundation.md](plans/phase0-foundation.md) 执行阶段 0 的细化步骤（阶段 A~F，共 30 个最小功能单元），建立可编译、可测试、可 CI 的 Rust workspace 骨架。
