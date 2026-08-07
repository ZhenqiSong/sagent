# sagent

模块化的本地优先 AI Agent Runtime，采用 Rust 从零重写。

## 项目定位

- **独立 Rust 项目**，不兼容 Python 版 Hermes Agent 的配置/数据库格式。
- **不兼容**旧 Python 模块结构、旧 SQLite schema、Python 插件 ABI。
- 目标为单二进制、低资源占用、生产级部署。

## 参考项目

当前 Python 版 Hermes Agent（`hermes-agent`）仅作为行为参考，用于理解已有协议语义和边界条件。Sagent 不依赖 Python 运行时、不导入 Python 模块、不复用旧数据格式。

## 开发阶段

当前处于 **Phase 0：项目基础与协议设计**。Phase 0 的目标是建立工程骨架和协议基线，不包含 Agent Loop、模型调用、工具执行或数据库。

详细实施计划见 [plans/sagent-phase0-implementation-guide.md](plans/sagent-phase0-implementation-guide.md)。

## 构建

```bash
cargo check --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

## 许可证

MIT
