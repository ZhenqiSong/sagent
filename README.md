# sagent

模块化的本地优先 AI Agent Runtime，采用 Rust 从零重写。目标为单二进制、低资源占用、生产级部署。

## 项目定位

- **独立 Rust 项目**，不兼容 Python 版 Hermes Agent 的配置/数据库格式
- **不兼容**旧 Python 模块结构、旧 SQLite schema、Python 插件 ABI
- 协议优先、核心窄腰、本地优先、安全默认

## 快速开始

```bash
# 编译
cargo build --release

# 启动 stdio JSON-RPC server
echo '{"jsonrpc":"2.0","id":"1","method":"rpc.echo","params":{"value":"hello"}}' \
  | cargo run --bin sagent -- rpc stdio
# → {"jsonrpc":"2.0","id":"1","result":{"value":"hello"}}

# 查看协议版本
printf '{"jsonrpc":"2.0","id":"1","method":"protocol.describe","params":{}}\n' \
  | cargo run --bin sagent -- rpc stdio
# → {"jsonrpc":"2.0","id":"1","result":{"protocol":"sagent.rpc","version":1,...}}
```

## 架构

```text
sagent-types             # 零 IO 依赖，纯数据模型（Message/ToolCall/Event/ID）
    ^
sagent-api               # JSON-RPC 协议层（Request/Response/Error/Schema/Logging/Paths）
    ^
sagent (binary)          # CLI 入口（clap + stdio transport + dispatcher）
```

| Crate | 依赖 | 职责 |
| --- | --- | --- |
| `sagent-types` | `serde`, `serde_json` | 核心数据类型（Message, ToolCall, ToolDefinition, Event, ProtocolVersion） |
| `sagent-api` | `sagent-types`, `tracing` | JSON-RPC 类型、错误码、Schema 生成、日志初始化、路径解析 |
| `sagent` (bin) | `sagent-types`, `sagent-api`, `clap` | CLI 入口、stdio transport、方法分发 |

## CLI 命令

```bash
sagent rpc stdio                    # 启动 stdio JSON-RPC server
sagent protocol generate-schemas    # 生成 JSON Schema 到 protocols/schemas/
```

**Phase 0 方法**：`rpc.echo`（echo 验证）、`protocol.describe`（协议版本查询）、`health.get`（健康检查）

## 构建与测试

### 本地验收

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny check
cargo audit
```

### 测试覆盖（164 tests + 4 doc-tests）

| 类别 | 测试数 | 说明 |
| --- | --- | --- |
| 类型序列化 | 50 | Message/ToolCall/Event/ID round-trip |
| Schema 一致性 | 34 | 4 schema + 31 fixture 正反向校验 |
| Dispatcher 单元 | 19 | 方法分发、错误码、notification、边界校验 |
| stdio 端到端 | 18 | 子进程请求-响应周期和超限输入 |
| Request 类型单元 | 1 | 缺省 params 反序列化 |
| 日志端到端 | 11 | stdout/stderr 隔离、敏感数据过滤 |
| 路径单元 | 14 | SAGENT_HOME 覆盖、平台默认路径 |
| 日志单元 | 17 | 幂等初始化、敏感字段脱敏 |

### CI 矩阵

Linux / macOS / Windows 三平台，含 fmt → check → test → clippy → deny → audit 全流程，任一步失败即 CI 失败。

## 协议

当前协议版本：**`sagent.rpc` v1**，基于 JSON-RPC 2.0，newline-delimited JSON over stdio。

| 协议资产 | 数量 | 路径 |
| --- | --- | --- |
| JSON Schema | 4 | `protocols/schemas/` |
| Valid fixtures | 15 | `protocols/fixtures/valid/` |
| Invalid fixtures | 18 | `protocols/fixtures/invalid/` |
| 错误码 | 10 | 标准 5 个 + 扩展 5 个 |

详细协议文档见 [docs/protocol-v1.md](docs/protocol-v1.md)。

## 配置

`~/.sagent/`（`SAGENT_HOME` 可覆盖）为本项目数据目录：

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

行为配置在 `config.yaml`，密钥在环境变量/`secrets.env`。示例配置见 [config.example.yaml](config.example.yaml)。

## 开发阶段

| Phase | 状态 | 内容 |
| --- | --- | --- |
| **Phase 0** | ✅ 完成 | 项目基础与协议设计 |
| Phase 1 | 🔜 待开始 | 基础设施（sagent-config, sagent-session, SQLite） |
| Phase 2 | 📋 计划中 | Agent 内核（Agent Loop, Session Actor, Tool Registry） |
| Phase 3+ | 📋 计划中 | 可靠性与安全、子代理/Skills/MCP、前端接入、生态扩展 |

Phase 0 交付检查表见 [docs/phase0-checklist.md](docs/phase0-checklist.md)，完整实施计划见 [plans/sagent-phase0-implementation-guide.md](plans/sagent-phase0-implementation-guide.md)。

## 设计原则

1. **协议优先** — Runtime/CLI/TUI/Desktop/插件通过稳定 JSON-RPC 协议交互
2. **核心窄腰** — Agent Loop/Session/Tool/Provider/Event 是核心，具体能力放边缘
3. **本地优先** — 数据存 `~/.sagent`，支持离线运行
4. **Prompt Cache 安全** — Session 内系统提示和工具集合保持 byte-stable
5. **取消优先** — 所有长任务可取消（`CancellationToken`）
6. **安全默认** — 文件/终端/插件默认受限
7. **模块化单体** — 内部 Cargo workspace 解耦，初期不拆微服务

## 文档

| 文档 | 说明 |
| --- | --- |
| [docs/protocol-v1.md](docs/protocol-v1.md) | 协议 v1 完整规范 |
| [docs/logging.md](docs/logging.md) | 日志系统（tracing、敏感数据过滤） |
| [docs/paths.md](docs/paths.md) | 路径规则（SAGENT_HOME、目录结构） |
| [docs/non-goals.md](docs/non-goals.md) | Phase 0 明确不做的功能 |
| [docs/phase0-checklist.md](docs/phase0-checklist.md) | Phase 0 交付检查表 |
| [protocols/README.md](protocols/README.md) | 协议目录说明 |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 贡献指南 |
| [CODEBUDDY.md](CODEBUDDY.md) | AI 助手指南 |

## 许可证

MIT
