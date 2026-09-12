# Sagent 重构护栏与可读性基线

记录日期：2026-09-13  
适用范围：Phase 0–2 已实现的 Rust 代码  
用途：为 `rust-code-quality-and-architecture-optimization-plan.md` 的 R0–R8 提供重构前
的可观察行为、结构体量与已知风险基线。本文件不是功能规格，也不以 Python/Hermes 为
行为 oracle。

## 1. 重构前必须守住的行为

以下行为由版本化 contract、集成测试或端到端测试共同保护。重构不得修改其输入、终态、
事件顺序、权限边界或持久化可见性；若业务确需变更，必须先更新对应设计与契约。

| 行为边界 | 当前证据 | 必须保持的契约 |
|---|---|---|
| RPC hello/capability | `contracts/rpc/client-hello-tui.json` | 协议版本、feature 集合、client capability 与 busy policy 的正常化结果 |
| Transcript 与工具轮次 | `contracts/transcript/tool-roundtrip.json` | role 顺序、tool call ID 关联和 assistant/tool 交替 |
| 工具 registry/policy | `contracts/tools/*.json` | schema、hash、工具名排序及危险命令的审批/拒绝分类 |
| Profile 与公开配置 | `contracts/config/profile-name.json`、config/RPC tests | Profile 名规范化、隔离和公开配置不泄露密钥 |
| SQLite 初始结构 | `contracts/store/fresh-schema.json`、Store tests | schema version、FTS 可用性、只读边界和迁移语义 |
| 提交、流式事件与终态 | `crates/sagent-runtime/tests/provider_actor.rs` | delta/usage 与持久化终态分离；成功、失败或中断只能有一个终态 |
| 工具、审批与取消 | `crates/sagent-runtime/tests/tool_actor.rs`、`fault_matrix.rs` | 先持久化计划再执行；审批前不启动危险工具；迟到结果不能复活 Turn |
| 崩溃恢复 | `crates/sagent-runtime/tests/recovery_actor.rs` | 未完成 Turn fail-closed，不伪造 assistant 终态 |
| RPC 连接 | `crates/sagent-rpc/tests/stdio_integration.rs`、WebSocket tests | response/event 顺序、连接隔离、非法帧拒绝和 EOF 收口 |
| TUI 黑盒 | `crates/sagent-tui/tests/blackbox_e2e.rs` | 真实进程下的提交、审批、Ctrl-C、重连和 transcript 唯一性 |
| 原生终端进程树 | `crates/sagent-tools/tests/terminal.rs` | Windows Job Object、Unix process group、timeout/cancel 后无活动受监管进程 |

执行全部版本化行为契约：

```powershell
cargo run -p sagent-contracts
```

涉及 Runtime、Store、工具或 RPC 的重构至少还须运行相关集成测试；只比较 JSON fixture
不能覆盖实际任务调度、SQLite 事务或进程生命周期。

## 2. 结构与文档快照

以下数字来自当前工作树的 `.rs` 文件静态统计，用作 review 触发信号而非硬性 lint。
统计包含 crate 内测试模块；函数长度使用大括号匹配的近似测量，宏展开和条件编译不计入。

| 指标 | 基线 |
|---|---:|
| Rust 文件数 | 138 |
| Rust 总行数 | 32,055 |
| 生产文件估计行数 | 26,060 |
| 独立测试文件估计行数 | 5,995 |
| 超过 500 行的 Rust 文件 | 14 |
| 超过 800 行的 Rust 文件 | 3 |
| 函数数（近似） | 1,138 |
| 超过 100 行的函数 | 20 |
| 超过 200 行的函数 | 0 |
| `#[test]` / `#[tokio::test]` | 359 |
| public 声明（近似） | 544 |
| 紧邻 Rustdoc 的 public 声明 | 488（约 89.7%） |

重点阅读/拆分候选：

| 文件或函数 | 当前规模 | 原因 |
|---|---:|---|
| `sagent-runtime/tests/tool_actor.rs` | 994 行 | 多种工具、审批和取消场景共享重复 setup |
| `sagent-runtime/src/supervisor.rs` | 828 行（测试自约 387 行开始） | 生命周期与全局能力组装需分离 |
| `sagent-rpc/src/transport/stdio.rs` | 806 行（测试自约 652 行开始） | framing、连接、dispatch、event bridge 混合 |
| `sagent-runtime/src/actor.rs` | 760 行 | submit、恢复、取消与状态机路径需要按主题阅读 |
| `sagent-store/src/turn.rs` | 623 行 | Turn 开始、工具提交、完成等事务集中 |
| `handle_worker_event` | 约 168 行 | 流、工具、Store 与事件发布混合 |
| `execute_with_approval` | 约 160 行 | 校验、审批、spawn、取消、输出和清理混合 |
| TUI `reduce` | 约 155 行 | 多个 UI 领域 action 集中 |

## 3. 当前依赖边界快照

```text
sagent-config ──► sagent-provider（当前不理想：解析层创建具体 Provider）
sagent-protocol ─► sagent-store   （当前不理想：协议层含具体 SessionService）
sagent-tools ────► sagent-store   （当前需在 Storage port 工作包中收口）
sagent-runtime ─► provider/store/tools/agent/types
sagent-rpc ──────► config/protocol/runtime/store/tools
sagent-tui ──────► protocol/types（当前边界正确）
```

本快照用于验证 R3–R6：目标不是让所有 crate 互不依赖，而是保证依赖从配置/协议/UI 等高层
指向稳定领域端口，而不是指向 SQLite、HTTP client、CLI 或 Runtime 的具体实现。

## 4. CI 与平台证据

CI 在 Windows、macOS、Linux 原生 runner 执行格式化、编译、测试、TUI 黑盒、contract、
原生 terminal 监督、Clippy 与 metadata；依赖审计另在 Linux 执行。

R0 起，quality matrix 还必须运行：

```powershell
cargo doc --workspace --no-deps
```

该检查验证 crate 文档、public API 解析和 intra-doc link，不会自动强制 `missing_docs`。
存量公开 API 文档补齐后，才评估将 warning 提升为强制 gate。

已知风险：曾有一次 macOS 原生 terminal 监督 CI 失败，但之后最新三平台 CI 已成功。终端测试
失败时必须输出平台、测试阶段、耗时和仍被 `ProcessSupervisor` 登记的 shell PID；禁止输出
用户命令、环境变量或凭据。

## 5. R0 退出检查

- [x] 已将当前行为映射到 contract、集成或黑盒测试；
- [x] 已记录代码体量、文档覆盖和依赖方向快照；
- [x] 原生 terminal 测试失败时可输出安全的生命周期诊断；
- [x] CI 已加入 workspace 文档构建；
- [x] 已在当前分支运行并通过 `cargo fmt --all -- --check`、`cargo test --workspace --quiet`、
  `cargo run -p sagent-contracts`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo doc --workspace --no-deps` 与 `git diff --check`。

该记录来自本工作包变更后的本地验证；它不替代后续 PR 的三平台原生 CI。
