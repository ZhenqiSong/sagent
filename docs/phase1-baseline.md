# Phase 1 基线记录

本记录对应 Phase 1 实施指南 Step 0，用于确认 Phase 0 已完成且工作区可以作为
Phase 1 基础设施的起点。

## 基线

- 基线 commit：`a8fa87a72b9d16153baa5905c3c21ae66c67300a`
- Rust toolchain：`stable-aarch64-apple-darwin`
- `rustc`：`1.94.1 (e408947bf 2026-03-25)`
- `cargo`：`1.94.1 (29ea6fb6a 2026-03-24)`
- 记录日期：2026-08-14

## 验收结果

以下命令均已在仓库根目录执行并通过：

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo deny check`
- `cargo audit`

当前测试包含 164 个普通测试和 4 个通过的 doc-tests；被标记为 ignored 的
logging doc-tests 不影响基线验收。

## Phase 0 能力核对

- `sagent-types` 已提供 Message、ToolCall、ToolDefinition 和 ModelEvent。
- `sagent-api` 已提供 JSON-RPC request、response、error 和 event envelope。
- 已实现 `protocol.describe`、`health.get` 和 `rpc.echo`。
- `sagent_api::paths::SagentHome` 已提供路径解析和 `SAGENT_HOME` 覆盖。
- tracing subscriber 已将日志写入 stderr，并与 stdout 协议输出隔离。
- `.github/workflows/ci.yml` 已提供 Linux、macOS、Windows 质量检查和基础编译
  矩阵，并包含 schema、依赖和安全审计检查。

## Phase 1 越界检查

- 当前 workspace 仍只有 `sagent-types`、`sagent-api` 和 `bins/sagent`。
- 尚未增加 `sagent-session`、SQLite 或 `tokio::spawn`。
- 当前没有 Provider、Session、Agent Loop 或实际工具执行实现。
- 当前工作区的未提交修改只涉及 Phase 0 公共 API 命名和对应文档、测试；本步骤未
  覆盖或回滚这些修改。

## 协议状态

未发现需要在 Phase 1 代码中临时分叉的未决协议问题。Phase 1 新增的 Session
错误码、方法和事件契约必须继续遵循 `protocols/protocol-decisions.md`、
`docs/protocol-v1.md` 和现有 schema 生成规则。
