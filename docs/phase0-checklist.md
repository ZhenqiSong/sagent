# Phase 0 交付检查表

本文档跟踪 Phase 0 的所有交付物状态。所有项目均已完成。

## 仓库与基础

- [x] 独立 Sagent 仓库和 README
- [x] `docs/non-goals.md` — Phase 0 明确不做的功能列表
- [x] `protocols/reference-notes.md` — Python 参考代码阅读记录
- [x] `protocols/protocol-decisions.md` — 17 项协议设计决策
- [x] `Cargo.toml` 和 `Cargo.lock` — Workspace 配置
- [x] `rust-toolchain.toml` — 固定工具链
- [x] `.rustfmt.toml` — 统一格式化规则
- [x] `deny.toml` — 依赖审计配置
- [x] `CONTRIBUTING.md` — 贡献指南和验收命令
- [x] `.github/workflows/ci.yml` — Linux/macOS/Windows CI

## 核心 crate

- [x] `sagent-types` — 零 IO 依赖的公共数据类型（Message/ToolCall/ToolDefinition/Event/ID/Version）
- [x] `sagent-api` — JSON-RPC 协议类型和错误码（Request/Response/Error/Event/Schema）
- [x] `sagent-api/src/paths.rs` — SAGENT_HOME 路径解析模块
- [x] `sagent-api/src/logging.rs` — tracing 日志初始化（stderr，敏感数据过滤，幂等）

## 二进制入口

- [x] `bins/sagent` — CLI 入口
  - [x] `rpc stdio` — stdio JSON-RPC echo server
  - [x] `protocol generate-schemas` — schema 生成命令
  - [x] `dispatcher.rs` — 方法分发（rpc.echo / protocol.describe / health.get）

## 协议文档

- [x] `docs/protocol-v1.md` — 协议 v1 完整文档（请求/响应/事件/错误码/版本协商/日志/Schema）
- [x] `docs/logging.md` — 日志系统文档
- [x] `docs/paths.md` — 路径规则文档
- [x] `protocols/README.md` — 协议目录说明

## JSON Schema 与 Fixture

- [x] 4 个 JSON Schema 文件（Rust 代码生成，无手工漂移）
- [x] 15 个 valid fixtures
- [x] 18 个 invalid fixtures
- [x] Schema 生成命令：`cargo run --bin sagent -- protocol generate-schemas`

## 测试覆盖

| 类别 | 文件 | 数量 |
| --- | --- | --- |
| 类型序列化 | `crates/sagent-types/tests/serialization.rs` | 50 |
| Schema 一致性 | `crates/sagent-api/tests/schema_tests.rs` | 34 |
| Dispatcher 单元 | `bins/sagent/src/dispatcher.rs` | 19 |
| stdio 端到端 | `bins/sagent/tests/stdio_echo.rs` | 18 |
| Request 类型单元 | `crates/sagent-api/src/request.rs` | 1 |
| 日志端到端 | `bins/sagent/tests/logging_tests.rs` | 11 |
| 路径单元 | `crates/sagent-api/src/paths.rs` | 14 |
| 日志单元 | `crates/sagent-api/src/logging.rs` | 17 |
| Doc-tests | — | 4 |
| **总计** | | **164** |

## 验收命令

- [x] `cargo fmt --all -- --check` ✅
- [x] `cargo check --workspace --all-targets` ✅
- [x] `cargo test --workspace` ✅ (164 tests + 4 doc-tests)
- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings` ✅
- [x] `cargo deny check` ✅
- [x] `cargo audit` ✅
- [x] `cargo run --bin sagent -- protocol generate-schemas && git diff --exit-code -- protocols/schemas` ✅
- [x] CI schema 生成一致性检查 ✅
- [x] stdio 端到端：`printf ... | cargo run --bin sagent -- rpc stdio` ✅

## 审查结论

1. **sagent-types 窄腰** ✅ — 仅依赖 serde + serde_json，无 IO/HTTP/DB/CLI
2. **协议一致性** ✅ — Rust 类型、schema、fixture、文档四者一致
3. **stdout 协议隔离** ✅ — 所有 stdout 输出均为 JSON-RPC frame，日志走 stderr
4. **错误码稳定** ✅ — 每个错误码有唯一来源，所有构造方法有测试
5. **依赖合规** ✅ — 无 git 依赖、无通配符版本、无 Phase 1+ 依赖（tokio/reqwest/sqlx）
6. **无 Phase 1 越界** ✅ — 无 Provider/Session/SQLite/真实 Tool/模型调用
7. **Python 路径** ✅ — 无 `~/.hermes`、Python 模块名或旧表字段残留
8. **参考代码记录** ✅ — 覆盖 Agent、工具、JSON-RPC、Transport、路径、日志和协议测试入口
