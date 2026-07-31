# Sagent — 全量 Rust 重写计划书

> 范围：CLI / Agent 内核 / 工具系统 / Gateway（20+ 平台）/ TUI / Electron 桌面端 / 插件系统
> 兼容性：**全新独立项目，不兼容 Python 版**（独立命名、独立 `~/.sagent/` 路径、独立配置与数据库格式）
> 制定日期：2026-07-31

---

## 0. 总览

Hermes（即 sagent 的前身，Python 项目）当前是一个 ~12 万行级的 Python 单体项目（核心 `run_agent.py` ≈12k 行、`cli.py` ≈11k 行、`agent/` 177 个模块、`tools/` 80+ 工具、`gateway/platforms/` 20+ 适配器、Electron 桌面端为 TypeScript/React）。本次重写为**用 Rust 从零重建全部能力，命名为 sagent**，作为独立项目存在，不复用 Python 版的配置/数据库格式。

### 0.1 重写目标

| 目标 | 说明 |
|------|------|
| 性能 | 消除 Python GIL 与解释开销；并发会话、流式、PTY 桥接等用 `tokio` 原生异步 |
| 单二进制分发 | `sagent` CLI 编译为单一可执行文件，自带 Gateway/桌面能力，免 Python 运行时 |
| 内存安全 | 用 Rust 类型系统在编译期消除整类空值/并发/内存错误 |
| 可维护的窄腰核心 | 沿用 AGENTS.md 的设计哲学：core 是 narrow waist，能力在边缘（CLI+skill / service-gated / plugin / MCP） |
| 缓存与契约安全 | 严格保留 per-conversation prompt caching、message role alternation、byte-stable system prompt |

### 0.2 不重写、但保留契约的部分

- **Electron 前端**：保持 TypeScript/React（桌面 UI 层），仅将后端从 Python 进程替换为 Rust 进程，通过 **stdio IPC / 本地 HTTP** 通信。前端代码库（`apps/desktop/`）基本复用。
- **Skills 与 Prompt 内容**：skill 的 *语义与目录约定* 可重新设计，但交互契约（agent 如何发现/调用/审计 skill）须对齐，以保证能力可迁移。

---

## 1. 总体架构（Cargo Workspace）

采用 Cargo workspace 多 crate 结构，每个 crate 对应一个清晰的边界，避免 Python 单体"god-file"问题：

```
sagent/
├── Cargo.toml                 # workspace 根
├── crates/
│   ├── sagent-core/           # agent 内核：对话循环、provider adapters、prompt builder、
│   │                         #   prompt caching、context compression、tool dispatch、subsessions
│   ├── sagent-config/         # 配置解析、profile 管理、secrets 加密、路径管理
│   ├── sagent-store/          # SQLite 会话存储 (FTS5)、memory DB、项目 DB、kanban DB
│   ├── sagent-tools/          # 工具注册表 + CLI 命令 + skill 执行 + memory 工具
│   │   └── environments/      # 终端后端：local / docker / ssh / modal / daytona
│   ├── sagent-gateway/        # 消息网关核心 + 平台适配器 (20+)
│   ├── sagent-cli/            # CLI 编排器 + TUI (ratatui) + setup 向导
│   ├── sagent-plugins/        # 插件运行时（WASM via extism / 动态库）
│   ├── sagent-mcp/            # MCP client（stdio / SSE）+ OAuth
│   ├── sagent-browser/        # CDP 浏览器驱动
│   ├── sagent-proto/          # 共享类型：消息、工具 schema、平台枚举（serde）
│   └── sagent-common/         # 错误类型、日志(tracing)、i18n、redact、速率限制基元
├── desktop/                   # 复用现有 apps/desktop 的 TS 前端，仅改 IPC 绑定到 Rust
└── scripts/                   # 构建/测试/发布脚本
```

**核心设计锚点（必须贯穿所有阶段）**
1. **Prompt caching 是神圣的**：一次长对话每轮复用缓存前缀。Rust 侧要保证：system prompt 在对话生命周期内 byte-stable；tools schema 不随轮次变化；任何"上下文压缩"仅发生在显式触发时，且压缩后重新建立缓存前缀。
2. **窄腰原则**：新增 *模型工具* 的门槛极高。优先顺序：扩展现有代码 → CLI 命令 + skill → service-gated tool (`check_fn`) → 插件 → MCP catalog → 新 core tool（最后手段）。
3. **交替不变量**：消息序列绝不允许连续两条同角色消息，绝不在循环中注入合成 user 消息。
4. **行为契约测试**：断言两段数据间的不变量，而非冻结某个枚举值/版本字面量（避免 change-detector 测试）。

---

## 2. 技术选型总表（Python → Rust）

| Python 依赖 / 机制 | Rust 替代 | 用途 |
|---|---|---|
| `openai` SDK / 直连 REST | `reqwest` + `serde` + 自封 client（OpenAI/Anthropic 兼容） | LLM API 调用、流式 |
| `httpx[socks]` | `reqwest` (proxy/socks 特性) | HTTP / SOCKS 代理 |
| `asyncio` | `tokio` | 异步运行时 |
| `pydantic` | `serde` + `thiserror` | 数据校验与序列化 |
| `pyyaml` / `ruamel.yaml` | `serde_yaml` | 配置解析 |
| `rich` / `prompt_toolkit` | `ratatui` + `crossterm` + `dialoguer` | TUI / 交互式输入 |
| `fastapi` / `uvicorn` | `axum` | 本地 HTTP（dashboard / gateway webhook / desktop IPC） |
| `sqlite3` (FTS5) | `rusqlite` (+ `rusqlite::functions`) | 会话/memory 持久化 |
| `ptyprocess` / `pywinpty` | `portable-pty` | 跨平台 PTY 终端桥接 |
| `psutil` | `sysinfo` / `procfs` | 进程/PID 管理 |
| `websockets` | `tokio-tungstenite` | CDP / 平台 WS 长连接 |
| `cryptography` | `rustcrypto` (`aes-gcm`, `sha2`, `argon2`) | secrets 加密、WeCom/Weixin 签名 |
| `PyJWT` | `jsonwebtoken` | Skills Hub JWT 鉴权 |
| `croniter` | `cron` + 自写调度器 | cron 作业 |
| `Markdown` | `comrak` / `markdown` (rust) | markdown→HTML 富文本投递 |
| `tenacity` | 自写 retry 组合子 | 重试/退避 |
| `jinja2` | `tinytemplate` / `minijinja` | prompt 模板 |
| `python-dotenv` | 启动时读取 `.env` | 密钥加载（仅 secrets） |
| `pathspec` | `ignore` crate | gitignore 感知匹配 |

> 行为配置（超时/阈值/特性开关/显示偏好）**一律进 `config.yaml`**，不在 `.env`。`.env` 仅放密钥。这是 AGENTS.md 的硬性规则。

---

## 3. 分阶段计划

### 阶段 0 — 工程奠基（脚手架 / 共享内核 / 框架）

**目标**：搭好 workspace、CI、错误处理、日志、配置骨架、共享类型，为后续阶段立桩。

- [ ] Cargo workspace 初始化；`rust-toolchain.toml`（stable + clippy）、`deny.toml`（依赖审计）、`release.toml`。
- [ ] `sagent-common`：统一 `Error`（`thiserror`）、`tracing` 日志（agent.log / errors.log / gateway.log，profile 感知）、`redact`（敏感标识符遮蔽基元）、`i18n` 占位、速率限制 token bucket。
- [ ] `sagent-proto`：核心类型——`Message`（role 枚举 + content parts）、`ToolCall` / `ToolResult`、平台枚举 `Platform`、`SessionSource`、`SendResult`、`MessageEvent` / `MessageType`。
- [ ] `sagent-config`：加载 `config.yaml`（serde_yaml），profile 感知的 `get_sagent_home()` / 路径管理；`.env` 仅读密钥。配置 schema 重新设计（不兼容，但语义对齐 Python 版字段）。
- [ ] CI：clippy + fmt + `cargo audit` + 单元测试；跨平台矩阵（Linux/macOS/Windows）。

**验收**：`cargo build` 全 workspace 通过；`sagent --version` 打印版本；能解析一个样例 `config.yaml` 并输出路径。

---

### 阶段 1 — 配置与持久化层

**目标**：稳定的配置、profile、secrets 加密、SQLite 存储（会话 FTS5、memory、项目、kanban）。

- [ ] `sagent-config`：profile 体系（独立岛屿，刻意不做默认 profile 继承——沿用 Python 设计意图，仅提供 `--clone` 复制）、`config.yaml` 迁移/校验、`OPTIONAL_ENV_VARS` 元信息（驱动 setup 向导）。
- [ ] `sagent-store`：
  - 会话 DB（`SessionDB` 等价物）：消息存储、FTS5 全文检索、会话恢复/导出（md/html）。
  - memory DB、projects DB、kanban DB（对齐 `hermes_cli/projects_db.py`、`kanban_db.py`）。
- [ ] secrets 加密：AES-GCM + Argon2 KDF，密钥从 `.env`/keychain 派生；对齐 `credential_persistence.py`、`secret_scope.py`。
- [ ] 进程级单例与锁（避免多进程写同一 DB），对齐 `sqlite_runtime.py` / `sqlite_safe_read.py`。

**验收（行为契约）**：
- 同一 `SAGENT_HOME` 下并发两个进程写会话，无 SQLite 死锁/损坏（不变量）。
- 加密 secrets 写入后，明文不落盘；解密需正确口令（契约）。
- 会话导出 md/html 与原文消息一一对应（不变量）。

---

### 阶段 2 — Agent 内核

**目标**：重建 `run_agent.py`（`AIAgent` 对话循环）+ `agent/` 关键模块。

- [ ] `sagent-core` 对话循环：`conversation_loop.rs`——逐轮：构建消息 → 调 provider → 流式 → 处理 tool calls → 执行 → 回灌，直至终止。
- [ ] **Provider adapters**（多 backend 统一 trait `LlmProvider`）：OpenAI / Anthropic / Gemini / Bedrock / Vertex / Azure / DeepSeek / 本地（lmstudio）。对齐 `anthropic_adapter.py`、`bedrock_adapter.py`、`vertex_adapter.py`、`gemini_native_adapter.py`、`azure_identity_adapter.py`。
- [ ] **Prompt caching**：system prompt 与稳定前缀离线计算 cache_control；工具 schema 在对话内不可变；压缩时重建前缀。对齐 `prompt_caching.py`。
- [ ] **Context compression**：显式触发的 `conversation_compression.rs`，压缩后重建缓存前缀（保留消息交替不变量）。对齐 `context_compressor.py` / `context_engine.py`。
- [ ] **Prompt builder**：`minijinja` 模板 + `PLATFORM_HINTS`（平台格式化提示）。对齐 `prompt_builder.py`。
- [ ] **Tool dispatch**：`tool_dispatch_helpers.rs`、`tool_guardrails.rs`（危险命令审批、路径安全 `path_security.py` 等价）、`tool_result_classification.rs`。
- [ ] **消息卫生**：`message_sanitization.rs`（role 交替校验、合成消息拦截）、`redact` 集成、严格交替保证。
- [ ] **子代理 delegation / subsessions**：`subagent_lifecycle.rs`、`delegation_context.rs`、`moa_loop.rs`（mixture-of-agents）。
- [ ] 计费/用量：`account_usage.rs`、`billing_*.rs`、`rate_limit_tracker.rs`、`credits_tracker.rs`、`usage_pricing.rs`（模型价目表）。
- [ ] 标题生成、摘要、trajectory 记录。

**验收（不变量 + 契约）**：
- 给定固定 system prompt + tools schema + 输入，连续 N 轮对话的缓存前缀哈希恒定（prompt caching 安全）。
- 任意消息序列经 sanitize 后绝不出现连续同角色（交替不变量）。
- provider 适配：对每个 provider 跑 mock 流式回放，断言 tool_call 解析与 OpenAI/Anthropic 格式一致。
- 危险命令（rm -rf / 写系统目录）默认走审批门禁（guardrail 契约）。

---

### 阶段 3 — 工具系统与运行时环境

**目标**：重建 `tools/`、`model_tools.py`、`toolsets.py`、`agent/memory_*`、`skills_*`。

- [ ] `sagent-tools` 注册表：`toolsets.rs`（`_SAGENT_CORE_TOOLS` 等价）、`discover_builtin_tools()`、`handle_function_call()`。窄腰：core tools 仅保留 terminal / read_file / web_search / browser 等 fundamentals。
- [ ] 文件工具：`file_tools.rs`（read/write/edit）、`file_operations.rs`、`path_security.rs`、`ansi_strip.rs`。
- [ ] 终端工具：`terminal_tool.rs` + `environments/`（local / docker / ssh / modal / daytona / singularity）——基于 `portable-pty`。Windows PTY 桥接用 `portable-pty` 的 ConPTY 支持（对齐 `win_pty_bridge.py`）。
- [ ] 浏览器工具：`sagent-browser`（CDP，基于 `tokio-tungstenite` + `chromedriver`/CDP 协议），`browser_tool.rs` / `browser_cdp_tool.rs` / `browser_supervisor.rs`。
- [ ] MCP 客户端：`sagent-mcp`（stdio / SSE transport）+ `mcp_oauth.rs`（OAuth 流）。catalog 优先于新增 core tool。
- [ ] Skills：`skill_commands.rs`、`skill_utils.rs`、`skill_preprocessing.rs`、`skills_hub.rs`、`skill_bundles.rs`、`skills_guard.rs`（AST 审计）、`skill_provenance.rs`。
- [ ] Memory：`memory_manager.rs`、`memory_provider.rs`（service-gated tool，仅当 memory 后端配置后暴露）。
- [ ] Agent 内建工具：delegate（子代理）、todo、clarify、approval（gateway 审批）、cronjob、send_message、session_search、skill_manager、kanban、vision、transcription、tts、image_gen、video_gen（service-gated，按后端配置暴露）。
- [ ] 安全：`tool_guardrails.rs`、`threat_patterns.rs`、`tirith_security.rs`、`url_safety.rs`、`file_safety.py` 等价。

**验收（契约）**：
- 工具 schema 在对话内稳定（缓存安全）；service-gated 工具仅在依赖满足时进入 schema。
- 路径安全：越权路径访问被拒绝（契约）。
- MCP catalog 工具与内置工具走同一 dispatch 路径（扩展点一致）。

---

### 阶段 4 — CLI 与 TUI

**目标**：重建 `cli.py`（`HermesCLI`）+ `hermes_cli/` 子命令 + curses TUI。

- [ ] `sagent-cli` 编排器：`clap` 子命令树，对齐 `hermes_cli/subcommands/`（44 个子命令）、`setup.py`、`gateway.py`、`profiles.py`、`models.py`、`mcp_*.py`、`plugins_cmd.py`、`skills_hub.py`、`cron.py`、`session_*.py`、`status.py`、`doctor.py`、`update_cmd.py` 等。
- [ ] 交互式 CLI：`dialoguer` 提示 + `ratatui` 渲染富输出（对齐 `console_engine.py`、`colors.py`、`cli_output.py`）。
- [ ] TUI：用 `ratatui` 重建 `curses_ui.py` 的会话视图 / focus pane / 多面板。
- [ ] Setup 向导：`gateway.py` 的 `_PLATFORMS` 列表、`model_setup_flows.py`、`memory_setup.py`、provider catalog。
- [ ] 平台无关 PTY 输入桥接（对齐 `pty_bridge.py` / `pty_session.py`）。

**验收**：`sagent setup` 引导配置 provider + 平台；`sagent chat` 进入交互会话；`sagent status` 显示平台/模型/资源。

---

### 阶段 5 — Gateway（核心 + 20+ 平台适配）

**目标**：重建 `gateway/run.py` + `gateway/platforms/*` + `gateway/session.py`。

- [ ] `sagent-gateway` 核心：`BasePlatformAdapter` trait；消息入站 dispatch（`handle_message` → `MessageEvent`）、授权映射（`_is_user_authorized`）、`SessionSource`、media cache、reconnect（指数退避+jitter）。
- [ ] 平台枚举 `Platform` + 适配器工厂 `_create_adapter()` + 授权 env 映射（`platform_env_map` / `platform_allow_all_map`）。
- [ ] 内置适配器（按 Python 版覆盖，至少首批）：Telegram、Discord、Slack、WhatsApp (Cloud + Baileys 混合)、Signal、Matrix、LINE、WeChat/Weixin、QQBot、BlueBubbles、Microsoft Graph、Webhook、yuanbao 等 —— 实现 `connect/disconnect/send/send_typing/send_image/get_chat_info` 及可选 `send_document/voice/video`、交互式 `send_clarify/send_exec_approval/send_slash_confirm/send_model_picker/send_choice_picker`。
- [ ] 交互回调约定：按钮 id 约定 `cl:<id>:<idx>`、`appr:<id>:<choice>`、`sc:<choice>:<id>` 跨适配器共享（对齐现有约定，保证 gateway resolver 通用）。
- [ ] 平台 hints 注入 prompt（`PLATFORM_HINTS`）。
- [ ] cron 投递 `platform_map`、send_message 路由 `_send_to_platform`、channel directory、status 显示、setup 向导平台项、redact 正则。
- [ ] Webhook receiver（FastAPI/axum）+ 平台 webhook 验证。

**验收（契约，对照 ADDING_A_PLATFORM.md 的 16 步）**：
- 每个平台：枚举存在且值正确；env 加载；adapter init（allowlist/默认）；helper 单测；SessionSource round-trip；授权集成；send_message 路由命中。
- 自消息过滤、回声过滤防回复循环（不变量）。
- 敏感标识符在所有日志被 redact（契约）。

---

### 阶段 6 — 插件系统（Rust 原生）

**目标**：重建 `plugins/`（loader、skills/providers/notifiers 类别）为 Rust 友好形态。

- [ ] 插件发现：`~/.sagent/plugins/` + pip entry point 等价（改为 manifest + 动态库 / WASM）。
- [ ] **运行时选型（二选一，建议 WASM）**：
  - **WASM via extism**：沙箱化、跨语言、安全边界清晰（推荐，契合"窄腰 + 边缘能力"哲学）。
  - **动态库 via `libloading`**：零开销但有 unsafe 边界，仅用于受信任的第一方插件。
- [ ] 插件类别 ABC：platform adapter、skill provider、memory backend、notifier、tool provider。
- [ ] 平台插件可零改动 core 注册（对齐 `ADDING_A_PLATFORM.md` 的 Plugin Path）。

**验收**：一个示例 WASM 平台插件能注册并收发消息，无需改动 core 代码。

---

### 阶段 7 — Electron 桌面端（前端复用 + Rust 后端桥接）

**目标**：保留 `apps/desktop/` 的 TS/React 前端，后端从 Python 换为 Rust。

- [ ] Rust 侧暴露 IPC/HTTP：本地 axum 服务 + stdio JSON-RPC，供 Electron 主进程调用。
- [ ] 前端改造：将原来调用 Python 子进程的部分改为调用 `sagent` 二进制的 IPC/HTTP（复用 `bootstrap-installer/` 的打包逻辑）。
- [ ] dashboard：文件管理器 multipart 上传、session 视图、浏览器预览、voice 模式 UI 复用。
- [ ] 桌面 SSH Windows 远程运行时（对齐 `windows_ssh_runtime.py`）、PTY 桥接在 Rust 侧实现。

**验收**：`npm run dev` 启动桌面端，能与 Rust 后端对话、查看会话、用浏览器工具。

---

### 阶段 8 — 集成测试、性能、发布

**目标**：质量门禁 + 发布通道。

- [ ] **E2E 验证**（对照 AGENTS.md：真实路径、真实 import、临时 `SAGENT_HOME`）：provider 流式、gateway 端到端（mock 平台 API）、tool 执行、压缩、delegation、cron 投递。
- [ ] 不变量测试套件：交替、缓存前缀稳定、路径安全、回复循环防护。
- [ ] 性能基线：并发会话吞吐、流式首 token 延迟、内存占用（对比 Python 版作为参考）。
- [ ] 发布：单二进制 `cargo-dist`；跨平台 installer（复用 `bootstrap-installer/`）；`sagent update` 自更新通道。
- [ ] 文档：README、平台适配指南（重写为 Rust trait 教程）、开发者指南。

**验收**：全 workspace `cargo test` + E2E 绿；单二进制在干净机器（无 Rust/Python 运行时）可运行 Gateway + CLI + 桌面。

---

## 4. 测试策略（对齐 AGENTS.md）

| 原则 | Rust 侧落实 |
|------|-------------|
| 行为契约 > 快照 | 用 `#[test]` 断言不变量（交替、缓存前缀哈希、路径拒绝），不 freeze 枚举计数/版本字面量 |
| E2E 真实路径 | 临时 `SAGENT_HOME`，真实 SQLite + 真实 provider mock server（wiremock） |
| 缓存/交替/不变量安全 | `conversation_loop` 单测构造消息序列，断言 sanitize 后交替成立、缓存前缀恒定 |
| 不引入 change-detector | 模型列表/配置版本用 schema 校验，不用具体字面量断言 |

---

## 5. 风险与缓解

| 风险 | 缓解 |
|------|------|
| 工作量巨大（~12 万行） | 严格按依赖顺序自底向上；每阶段可独立编译/测试；优先核心路径（CLI+agent+terminal+file+一个平台）达到 MVP |
| Python 生态库无 1:1 Rust 等价（如部分平台 SDK 仅 Python） | 用 REST/WS 直连替代官方 SDK；WASM 插件允许用任意语言写平台适配 |
| prompt caching 跨语言语义差异 | 阶段 2 即建立缓存前缀哈希测试，作为回归护栏 |
| 前端（TS）与 Rust 后端契约漂移 | 用 `sagent-proto` 生成 TS 类型（serde → ts-codegen），单一真相源 |
| 桌面端 PTY/SSH 跨平台坑（尤其 Windows） | 阶段 3/7 用 `portable-pty` ConPTY + 早期 Windows CI 验证 |
| 供应链安全（Python 版曾遇 PyPI 投毒） | 用 `cargo-deny` + 锁定 `Cargo.lock`；依赖最小化；secrets 不进依赖 |

---

## 6. 工作量估算与时间线（粗粒度）

| 阶段 | 量级（人月，单核估算） | 优先级 |
|------|------|------|
| 0 奠基 | 1 | P0 |
| 1 配置/持久化 | 1.5 | P0 |
| 2 Agent 内核 | 4 | P0 |
| 3 工具/环境 | 4 | P0 |
| 4 CLI/TUI | 2.5 | P1 |
| 5 Gateway(20+平台) | 5（平台可并行） | P1 |
| 6 插件 | 1.5 | P2 |
| 7 桌面端 | 2 | P2 |
| 8 测试/发布 | 2 | P1 |
| **合计** | **≈24 人月** | |

**MVP 里程碑**（阶段 0–3 + 单个 gateway 平台）：可在终端跑 agent、执行 terminal/file/web/browser 工具、接 Telegram 收发消息。建议以此为第一个可演示切片，再横向铺开其余平台与桌面端。

> 注：以上为单开发者顺序估算；阶段 5 的 20+ 平台可由多人/多 agent 并行（每个平台适配自包含），阶段 7 可与阶段 4/5 并行。
