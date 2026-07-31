## 用户需求

为 Hermes Agent（Python 项目，当前工作区 `d:/projects/hermes-agent`）的全量 Rust 重写中的"阶段 0 — 工程奠基"提供详细实施计划。用户已确认：**全量重写、完全独立不兼容 Python 版、新 workspace 创建在 `d:/projects/sagent/`**（实际目录，不含 `-rs` 后缀）。

## 阶段 0 目标

搭建可编译、可测试、可 CI 的 Rust workspace 骨架，为后续 7 个阶段提供统一的错误处理、日志、共享类型和配置基础设施。

## 阶段 0 验收标准

| 编号 | 验收项 | 验证方式 |
| --- | --- | --- |
| AC-0 | `cargo build --workspace` 全部通过 | 零错误零警告编译（clippy pedantic 通过） |
| AC-1 | `sagent --version` 打印 `sagent 0.19.1` | 运行编译产物验证 |
| AC-2 | CLI 能读取样例 `config.example.yaml` 并输出解析后的 `sagent_home` 路径 | 对照 Python 版路径逻辑验证 |
| AC-3 | CI 矩阵覆盖 `linux / macos / windows` 三个平台 | GitHub Actions workflow 绿灯 |
| AC-4 | 日志文件写到正确的 `<SAGENT_HOME>/logs/agent.log` 路径 | 临时目录 + tracing-test 验证 |


## 技术栈选型

| 层级 | 选型 | 理由 |
| --- | --- | --- |
| 构建系统 | Cargo workspace + `cargo-hakari` | 统一 workspace 依赖解析，减少编译时间 |
| 异步运行时 | `tokio` (full features) | 后续所有 I/O（HTTP/WS/PTY）均基于 tokio；阶段 0 仅引入但不使用 |
| 错误处理 | `thiserror` + `anyhow` | 库 crate 用 thiserror 派生枚举，二进制用 anyhow |
| 序列化 | `serde` + `serde_yaml` + `serde_json` + `serde_with` | YAML 配置、JSON 工具参数、snake_case 自动转换 |
| 日志 | `tracing` + `tracing-subscriber` + `tracing-appender` | 结构化日志、异步文件滚动、跨平台兼容 |
| CLI | `clap` (derive) | 类型安全的子命令解析，后续阶段逐步扩展子命令树 |
| 环境变量 | `dotenvy` | 加载 `<SAGENT_HOME>/.env`，仅用于 secrets |
| 敏感信息遮蔽 | `regex` | redact 模块用 Regex 匹配 token/phone/email 模式 |
| 跨平台路径 | `dirs` (platform-dirs) | 统一 Windows/POSIX 默认路径解析 |
| CI | GitHub Actions + `cargo-deny` + `cargo-audit` | 依赖审计、clippy lint、多平台编译矩阵 |


## 实施策略

采用 **自底向上依赖顺序**，且**每个步骤只实现一个最小可编译可验证的功能单元**。每完成一个功能点立即 `cargo build`（仅该 crate 或依赖它的部分）确认零错误零警告，再进入下一个。绝不一次写完整个 crate 再编译。

逐步推进顺序：

1. **Workspace 骨架**（根 `Cargo.toml` + `rust-toolchain.toml` + 空 crate 占位）→ 编译通过
2. **`sagent-common` 逐文件**：

- `error.rs`：`FailoverReason` 枚举 + `as_str()` → 编译
- `error.rs`：`ClassifiedError` 结构体 → 编译
- `error.rs`：`SagentError` 顶层错误（thiserror）+ `#[cfg(test)]` → 编译 + 单测绿
- `rate_limiter.rs`：`TokenBucket` → 编译 + 单测
- `i18n.rs`：占位枚举 → 编译
- `redact.rs`：`Redact` trait + regex 模式 → 编译 + 单测
- `logging.rs`：`setup_logging` + `RedactingLayer` → 编译
- `lib.rs`：重导出四个模块 → 编译

3. **`sagent-proto` 逐文件**（依赖 common）：

- `message.rs`：`Role`/`ContentPart`/`Message` → 编译 + serde 单测
- `usage.rs`：`Usage`/`NormalizedResponse`/`FinishReason` → 编译
- `tool.rs`：`ToolCall`/`ToolResult`/`ToolDefinition`/`ToolCallGuardrailConfig` → 编译
- `platform.rs`：`Platform` + `KnownPlatform` 24 成员 → 编译 + 单测
- `session.rs`：`SessionSource`/`SessionKey`/`ChatType` → 编译
- `gateway.rs`：`MessageType`/`MessageEvent`/`SendResult`/`ProcessingOutcome` → 编译
- `lib.rs`：重导出 → 编译

4. **`sagent-config` 逐文件**（依赖 proto）：

- `paths.rs`：`get_sagent_home` 三级优先级 → 编译 + 跨平台单测
- `paths.rs`：`get_config_path`/`get_env_path`/`get_logs_dir`/`get_skills_dir` → 编译
- `secrets.rs`：`load_env` → 编译
- `config.rs`：`SagentConfig` 结构 + `from_file` → 编译 + 单测
- `lib.rs`：重导出 → 编译
- `config.example.yaml`：样例文件 → 验证解析

5. **`sagent-cli`**：`main.rs`（`--version` / `version` / `config path`）→ 编译 + 运行验证
6. **集成测试** `tests/integration/config_roundtrip.rs` → 编译 + 测试绿
7. **CI** `.github/workflows/ci.yml` + `deny.toml` → 提交触发

每个功能点都对应下方「细化步骤」中的一个编号条目。

**关键设计决策**：

1. **错误类型对齐 Python 版语义**：`FailoverReason` 的 32 个枚举值直接 1:1 映射到 Rust enum，`.as_str()` 方法返回与 Python 版一致的短横线命名（如 `"auth_permanent"`），确保后续阶段重试/降级逻辑语义准确。`ClassifiedError` 在 Rust 侧设计为 trait method 的返回值而非异常的替代——`fn classify(&self) -> ClassifiedError` 方法挂在各 adapter error 类型上。

2. **日志采用 tracing 生态而非 Python 结构直译**：用 `tracing_subscriber::fmt::Layer` 的 `with_filter` 实现组件级前缀过滤，替代 Python 版的自定义 `_ComponentFilter`。用 `tracing_appender::non_blocking` 替代 Python 版的 `QueueListener` + `RotatingFileHandler`。日志文件路径 `<SAGENT_HOME>/logs/agent.log` 在 `sagent-config` 解析 home 后传入 `sagent-common::logging::setup()`。

3. **Platform 枚举支持动态扩展**：Rust 原生 `enum` 不允许运行时添加变体。使用一个内部 `KnownPlatform` 枚举（24 个已知成员）+ `Platform(String)` 包装类型，对外暴露 `serde` 序列化/反序列化。已知平台通过 `FromStr` + `Into<&str>` 做零开销转换；未知平台（插件动态注册）保留原始字符串值，保证 MCP/插件场景的扩展性。即：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnownPlatform {
    Local, Telegram, Discord, Slack, Whatsapp, WhatsappCloud,
    Signal, Mattermost, Matrix, Homeassistant, Email, Sms,
    Dingtalk, ApiServer, Webhook, MsgraphWebhook, Feishu,
    Wecom, WecomCallback, Weixin, Bluebubbles, Qqbot, Yuanbao,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Platform(String);

impl Platform {
    pub fn known(v: KnownPlatform) -> Self;
    pub fn new_dynamic(name: &str) -> Self;
    pub fn as_str(&self) -> &str;
    pub fn as_known(&self) -> Option<KnownPlatform>;
}
```

4. **CI 跨平台矩阵尽早建立**：阶段 0 就加入 `ubuntu-latest / macos-latest / windows-latest` 三个 runner，确保 `sagent-config::get_sagent_home()` 的 Windows `%LOCALAPPDATA%` 路径解析在早期发现 bug。

## 目录结构

```
d:/projects/sagent/
├── Cargo.toml                      # [NEW] workspace root: members = crates/*, workspace.dependencies
├── rust-toolchain.toml             # [NEW] channel = "stable", components = ["clippy", "rustfmt"]
├── deny.toml                       # [NEW] cargo-deny config: license allowlist, advisory db
├── config.example.yaml             # [NEW] 最小样例配置，用于 AC-2 验证
├── .github/
│   └── workflows/
│       └── ci.yml                  # [NEW] CI: build + clippy + fmt + test + audit; matrix: linux/macos/windows
├── crates/
│   ├── sagent-common/
│   │   ├── Cargo.toml              # [NEW] lib crate; deps: thiserror, tracing, tracing-subscriber, regex, anyhow
│   │   └── src/
│   │       ├── lib.rs              # [NEW] re-export: error, logging, redact, i18n, rate_limiter
│   │       ├── error.rs            # [NEW] SagentError enum + FailoverReason(32 values) + ClassifiedError + top-level errors
│   │       ├── logging.rs          # [NEW] setup_logging(mode, sagent_home): agent/errors/gateway log files, ComponentFilter, RedactingLayer
│   │       ├── redact.rs           # [NEW] Redact trait + RedactingLayer (tracing Layer impl) + regex patterns for tokens/phones/emails
│   │       ├── i18n.rs             # [NEW] stub: I18nKey enum placeholder
│   │       └── rate_limiter.rs     # [NEW] TokenBucket struct (capacity, refill_rate, last_check)
│   ├── sagent-proto/
│   │   ├── Cargo.toml              # [NEW] lib crate; deps: serde, serde_json, sagent-common
│   │   └── src/
│   │       ├── lib.rs              # [NEW] re-export all sub-modules
│   │       ├── message.rs          # [NEW] Role(Enum: System/User/Assistant/Tool), ContentPart::Text/Image, Message struct
│   │       ├── tool.rs             # [NEW] ToolCall, ToolResult, ToolDefinition(schema: Value), ToolCallGuardrailConfig
│   │       ├── platform.rs         # [NEW] Platform(String) wrapping KnownPlatform enum (24 members + FromStr + serde transparent)
│   │       ├── session.rs          # [NEW] SessionSource(20+ fields), SessionKey, ChatType enum(dm/group/channel/thread)
│   │       ├── gateway.rs          # [NEW] MessageType enum(10 variants: text/location/photo/video/audio/voice/document/sticker/command), MessageEvent, SendResult, ProcessingOutcome
│   │       └── usage.rs            # [NEW] Usage struct(prompt/completion/total/cached_tokens), NormalizedResponse, FinishReason enum
│   ├── sagent-config/
│   │   ├── Cargo.toml              # [NEW] lib crate; deps: serde, serde_yaml, serde_with, dirs, dotenvy, sagent-proto
│   │   └── src/
│   │       ├── lib.rs              # [NEW] re-export: paths, config, secrets
│   │       ├── paths.rs            # [NEW] get_sagent_home(), get_config_path(), get_env_path(), get_skills_dir(), get_logs_dir(); profile-aware; platform detection
│   │       ├── config.rs           # [NEW] SagentConfig struct: provider/models/gateway/tools/skills sections; load from config.yaml; merge .env secrets
│   │       └── secrets.rs          # [NEW] load_env(sagent_home): reads <home>/.env via dotenvy; validates secrets-only constraint
│   └── sagent-cli/
│       ├── Cargo.toml              # [NEW] bin crate; deps: clap, sagent-common, sagent-proto, sagent-config, anyhow
│       └── src/
│           └── main.rs             # [NEW] clap derive: `sagent --version` / `sagent version`; `sagent config path` subcommand for AC-2
└── tests/
    ├── integration/                # [NEW] workspace-level integration tests
    │   └── config_roundtrip.rs     # AC-2: parse config.example.yaml, assert sagent_home resolution
    └── e2e/                        # [NEW] placeholder for future E2E tests
```

## 关键代码结构

以上列出的 4 个 crate 共计 18 个源文件。以下为最关键的两个类型定义接口签名（不包含实现体）：

```rust
// crates/sagent-proto/src/platform.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnownPlatform {
    Local, Telegram, Discord, Slack, Whatsapp, WhatsappCloud,
    Signal, Mattermost, Matrix, Homeassistant, Email, Sms,
    Dingtalk, ApiServer, Webhook, MsgraphWebhook, Feishu,
    Wecom, WecomCallback, Weixin, Bluebubbles, Qqbot, Yuanbao,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Platform(String);

impl Platform {
    pub fn known(v: KnownPlatform) -> Self;
    pub fn new_dynamic(name: &str) -> Self;
    pub fn as_str(&self) -> &str;
    pub fn as_known(&self) -> Option<KnownPlatform>;
}
```

```rust
// crates/sagent-common/src/error.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailoverReason {
    Auth, AuthPermanent, Billing, RateLimit, UpstreamRateLimit,
    Overloaded, ServerError, Timeout, SslCertVerification,
    ContextOverflow, PayloadTooLarge, ImageTooLarge,
    ModelNotFound, ProviderPolicyBlocked, ContentPolicyBlocked,
    FormatError, InvalidEncryptedContent, MultimodalToolContentUnsupported,
    ThinkingSignature, LongContextTier, OauthLongContextBetaForbidden,
    LlamaCppGrammarPattern, Unknown,
}

#[derive(Debug, Clone)]
pub struct ClassifiedError {
    pub reason: FailoverReason,
    pub status_code: Option<u16>,
    pub retryable: bool,
    pub should_compress: bool,
    pub should_rotate_credential: bool,
    pub should_fallback: bool,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SagentError {
    #[error("SSL configuration error: {0}")] SslConfig(String),
    #[error("empty stream from provider")] EmptyStream,
    #[error("unknown MoA preset: {0}")] MoaPresetNotFound(String),
    #[error(transparent)] Io(#[from] std::io::Error),
    #[error("{0}")] Classified(ClassifiedError),
}
```

## 细化步骤（最小功能单元，逐步编译验证）

> 每个步骤完成即 `cargo build`（或 `cargo build -p <crate>`）确认零错误零警告，库模块附带 `#[cfg(test)]` 单测。

### 阶段 A：Workspace 骨架

- **A1** 创建 `d:/projects/sagent/Cargo.toml`（workspace root：`members = ["crates/*"]`，`workspace.dependencies` 统一定义 `thiserror`、`anyhow`、`serde`、`serde_yaml`、`serde_json`、`serde_with`、`tokio`、`tracing`、`tracing-subscriber`、`tracing-appender`、`clap`、`dirs`、`dotenvy`、`regex` 版本）
- **A2** 创建 `rust-toolchain.toml`（`channel = "stable"`，`components = ["clippy", "rustfmt"]`）
- **A3** 创建 `deny.toml`（`cargo-deny`：license allowlist MIT/Apache-2.0/BSD-3-Clause，advisory 启用）
- **A4** 创建 4 个空 crate 目录骨架（`sagent-common` / `sagent-proto` / `sagent-config` / `sagent-cli`），各自 `Cargo.toml` + 最小 `lib.rs`/`main.rs`（`sagent-cli` 仅 `fn main() {}`），使 `cargo build --workspace` 通过

### 阶段 B：`sagent-common`（逐文件）

- **B1** `error.rs` — `FailoverReason` 枚举（23 个成员见「关键代码结构」），`as_str()` 返回短横线命名；附单测断言 `as_str` 值
- **B2** `error.rs` — `ClassifiedError` 结构体（reason/status_code/retryable/should_compress/should_rotate_credential/should_fallback/message）+ `Default`/`new`
- **B3** `error.rs` — `SagentError`（`thiserror` 派生：`SslConfig`/`EmptyStream`/`MoaPresetNotFound`/`Io(#[from])`/`Classified`）；`From<ClassifiedError>`；附 `#[cfg(test)]`
- **B4** `rate_limiter.rs` — `TokenBucket { capacity, tokens, refill_per_sec, last_refill }` + `try_acquire()`/`acquire_blocking()`；附单测
- **B5** `i18n.rs` — 占位 `I18nKey` 枚举 + `t()` stub 函数（返回 `&'static str`，后续接真 i18n）
- **B6** `redact.rs` — `Redact` trait + `redact()` 默认实现；`regex` 编译 JWT/Slack token/API key/phone/email/UUID 模式；`redact_str(&str)` 公共函数；附单测覆盖每种模式
- **B7** `logging.rs` — `setup_logging(mode: &str, sagent_home: &Path) -> anyhow::Result<()>`；`tracing_appender` 非阻塞写入 `<home>/logs/{agent,errors,gateway}.log`；`RedactingLayer`（`tracing_subscriber::Layer` 实现，在 `on_event` 中 redact message 字段）
- **B8** `lib.rs` — 重导出 `error`/`logging`/`redact`/`i18n`/`rate_limiter` 模块

### 阶段 C：`sagent-proto`（依赖 common）

- **C1** `message.rs` — `Role`(System/User/Assistant/Tool) + `ContentPart::{Text,ImageUrl}` + `Message { role, content, tool_calls, tool_call_id, name }`（OpenAI 兼容 serde）；附 serde 往返单测
- **C2** `usage.rs` — `Usage { prompt_tokens, completion_tokens, total_tokens, cache_read_tokens, cache_write_tokens }` + `FinishReason` 枚举 + `NormalizedResponse { content, tool_calls, finish_reason, reasoning, usage }`
- **C3** `tool.rs` — `ToolCall { id, name, arguments, provider_data }` + `ToolResult { call_id, content, is_error }` + `ToolDefinition { name, description, schema: serde_json::Value }` + `ToolCallGuardrailConfig`（7 阈值字段）
- **C4** `platform.rs` — `KnownPlatform` 24 成员 + `Platform(String)`（`#[serde(transparent)]`）+ `FromStr`/`Display`/`as_str`/`known`/`new_dynamic`/`as_known`；附单测（已知+未知平台往返）
- **C5** `session.rs` — `ChatType`(dm/group/channel/thread) + `SessionSource`(platform/chat_id/user_id/chat_type/thread_id/guild_id/scope_id/profile 等 20+ 字段) + `SessionKey`；用 code-explorer 校验 Python `gateway/session.py` 字段清单
- **C6** `gateway.rs` — `MessageType`(text/location/photo/video/audio/voice/document/sticker/command 等 10 变体) + `MessageEvent { id, platform, source, msg_type, text, ... }` + `SendResult` + `ProcessingOutcome` 枚举；用 code-explorer 校验 `gateway/platforms/base.py`
- **C7** `lib.rs` — 重导出全部子模块

### 阶段 D：`sagent-config`（依赖 proto）

- **D1** `paths.rs` — `get_sagent_home()` 三级优先级（`SAGENT_HOME` env → Windows `%LOCALAPPDATA%\sagent` → POSIX `~/.sagent`）；附跨平台单测（用临时 env / mock）
- **D2** `paths.rs` — `get_config_path()`/`get_env_path()`/`get_logs_dir()`/`get_skills_dir()`（均 `<home>/...` 前缀）；profile 目录结构 `<home>/profiles/<name>/` + `<home>/active_profile` 读写
- **D3** `secrets.rs` — `load_env(sagent_home: &Path)` 经 `dotenvy` 读 `<home>/.env`；注释声明「仅密钥」约定
- **D4** `config.rs` — `SagentConfig` 顶层结构（provider/models/gateway/tools/skills 段落占位字段）+ `from_file(path)` + `merge_env_secrets()`；附单测解析样例
- **D5** `lib.rs` — 重导出 `paths`/`config`/`secrets`
- **D6** `config.example.yaml` — 最小样例（1 个 provider 占位 + 1 个 Telegram 平台占位）

### 阶段 E：`sagent-cli`

- **E1** `main.rs` — `clap` derive：`--version`（打印 `sagent 0.19.1`）、`version` 子命令、`config path` 子命令（调用 `get_config_path()` 输出）；启动时 `load_env`；`[[bin]] name = "sagent"`
- **E2** 手动运行验证 `sagent --version` 与 `sagent config path` 输出符合 AC-1/AC-2

### 阶段 F：测试与 CI

- **F1** `tests/integration/config_roundtrip.rs` — 断言 `SagentConfig::from_file("config.example.yaml")` 成功；断言 `get_sagent_home` 路径解析；断言 version 字符串格式
- **F2** `.github/workflows/ci.yml` — 矩阵 `ubuntu/macos/windows`；步骤 `build`/`clippy -D warnings`/`fmt --check`/`test`/`audit`
- **F3** 全量验证：`cargo build --workspace` + `cargo clippy -- -D warnings` + `cargo fmt --check` + `cargo test --workspace` + `cargo audit` 全绿（对应 AC-0~AC-4）

## Agent Extensions

### SubAgent

- **code-explorer**
- 用途：在实施过程中快速定位 Python 源码中需要对齐的类型定义、枚举值、字段清单（如 FailoverReason 的完整 32 个值、SessionSource 的 20+ 字段、Platform 的全量内置成员），避免手写遗漏
- 预期结果：每个 Rust 类型定义都经过与 Python 源码的交叉校验，字段命名和语义一致
