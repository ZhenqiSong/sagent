# AGENT.md

本文件是本仓库中 AI Agent 使用的开发指南，内容基于 `CODEBUDDY.md`，并补充当前 Phase 0 的实际边界。

## 项目概述

`sagent` 是独立的 Rust 本地优先 AI Agent Runtime，不兼容 Python Hermes Agent 的模块结构、旧 SQLite schema 或 Python 插件 ABI。

当前仓库处于 Phase 0：项目基础与协议设计。Phase 0 只提供：

- `sagent-types` 公共数据类型
- `sagent-api` JSON-RPC 类型、错误码、schema、日志和路径边界
- `sagent rpc stdio` 最小 newline-delimited JSON-RPC server
- `protocol.describe`、`health.get` 和 `rpc.echo`
- 协议 schema、fixture、conformance 测试和跨平台 CI

不得在 Phase 0 引入 Provider、HTTP/SSE、Session、SQLite、Agent Loop、实际工具执行、MCP、插件运行时、TUI、Desktop、Gateway 或 Scheduler。

## 目录与依赖边界

```text
sagent-types       # 纯数据模型；不得依赖 IO、Runtime、数据库、HTTP 或 CLI
    ^
sagent-api         # JSON-RPC 协议、错误、schema、日志、路径
    ^
bins/sagent        # CLI、stdio transport、方法分发
```

- `sagent-types` 是窄腰层，公共类型变更必须同步测试、fixture、schema 和文档。
- `sagent-api` 只负责协议边界，不实现 Session 或 Agent 业务。
- binary 负责启动、stdio 读写和依赖装配，不把日志写入 stdout。
- 不使用 git 依赖、通配符外部版本或未审计依赖。

## 常用命令

在仓库根目录执行：

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny check
cargo audit
```

生成并检查协议 schema：

```bash
cargo run --quiet --bin sagent -- protocol generate-schemas
git diff --exit-code -- protocols/schemas
```

运行 stdio server：

```bash
cargo run --quiet --bin sagent -- rpc stdio
```

stdio 约束：stdin 每行一个 JSON；stdout 每行一个 JSON-RPC frame 并立即 flush；stderr 只输出日志和诊断；EOF 正常退出；BrokenPipe 不得 panic。

## 协议约束

- JSON-RPC 版本固定为 `2.0`。
- request id 支持 string 和 integer，不把 `null` 当作可关联 request id。
- notification 不带 `id`，不返回 response。
- `params` 必须是 object，无参数使用 `{}`。
- response 必须且只能包含 `result` 或 `error` 之一。
- 协议 envelope 拒绝未知字段；业务 metadata 才允许扩展。
- 错误码必须在 `crates/sagent-api/src/error.rs` 统一定义。
- `protocol.describe` 的 feature 列表必须与实际注册方法一致。
- schema 的唯一生成来源是 `sagent-api/src/schema.rs`，禁止手工漂移。

协议变更必须同时更新：

- `protocols/protocol-decisions.md`
- `docs/protocol-v1.md`
- `protocols/schemas/`
- `protocols/fixtures/`
- 对应 Rust 序列化、schema 和进程测试

## 日志与安全

- 所有日志使用 `tracing` 写 stderr，不得污染 stdout。
- 默认日志级别为 `info`，可通过 `RUST_LOG` 覆盖。
- RPC 日志携带 `request_id`；不得输出完整 params、secret、API key、环境变量或绝对路径。
- 用户可见错误和代码注释使用简体中文；结构化日志字段名使用英文。
- 不在纯路径查询中隐式创建目录、数据库或 secrets 文件。

## 路径规则

路径必须通过 `sagent_api::paths::SagentHome` 获取，并使用 `PathBuf::join()`。

- `SAGENT_HOME` 可覆盖默认路径，但必须是绝对路径。
- 空值、相对路径和 NUL 字符的行为必须由测试和文档固定。
- 测试不得写入真实用户 home；使用显式测试根目录或进程隔离。
- 当前实现与 Phase 0 指南中的三平台默认路径仍需保持一致，修改路径规则时必须同步 `docs/paths.md` 和跨平台测试。

## 编码规范

- 每个 Rust 文件以模块级说明和作者、创建日期、变更记录开头，遵循 `CODEBUDDY.md` 的格式。
- 注释、文档注释和测试说明使用简体中文。
- 公共 API 添加文档注释。
- 避免无必要的抽象、兼容层和提前实现后续 Phase 功能。
- 不使用 `#[allow(clippy::*)]` 隐藏问题，除非有明确理由并写入代码说明。
- 测试优先验证行为不变量，不冻结无关的枚举计数或动态版本字面量。

## 修改前后检查

修改前先阅读相关 Rust 模块、协议决策、schema、fixture 和测试，并检查工作区状态。不要覆盖或回滚其他人的未提交修改。

修改后至少运行：

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
git diff --check
```

涉及 schema、协议或依赖时，额外运行 schema 生成检查、`cargo deny check` 和 `cargo audit`。

## Phase 0 验收状态

当前实现已覆盖：

- 超长 stdio 行、method 和 request id 返回 `-32003`，并测试超长行后继续处理。
- dispatcher 拒绝未知顶层 envelope 字段和非法 request id。
- invalid fixture 已覆盖 request、response 和 event schema 的反向校验场景。
