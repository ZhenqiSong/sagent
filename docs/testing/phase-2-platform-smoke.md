# Phase 2 平台 Smoke 记录

本文记录 Phase 2.5 的真实主机验证结果。Provider 请求只使用 loopback Mock SSE，
Profile、SQLite 和 workspace 使用临时目录，不读取开发机默认配置或真实凭据。

三平台闭环状态：已完成（Windows、macOS、Linux）。

## Windows

| 字段 | 值 |
| --- | --- |
| 日期 | 2026-09-12 |
| OS | Windows 11 家庭版中文版 10.0.26200，x64 |
| Shell/终端 | PowerShell 7.6.5，Windows ConPTY |
| Rust/Cargo | rustc 1.97.1，cargo 1.97.1 |
| 状态 | 已完成 Windows 原生可执行 smoke |

### 启动、退出与失败恢复

- 使用临时 `--home` 启动真实 `target/debug/sagent-tui.exe`，并由 TUI 启动真实
  `sagent-rpc.exe`；屏幕显示“已连接 RPC”，输入 `q` 后以退出码 0 返回。
- 使用不存在的 `--rpc-bin` 启动；程序返回退出码 1，输出“无法启动 RPC 子进程”，
  并发出光标恢复控制序列，当前 PowerShell 可继续输入。
- 退出后检查进程树，没有残留 `sagent-tui` 或 `sagent-rpc` 进程。

### 交互与工具路径

以下命令均在 Windows 原生 host 通过：

```text
cargo test -p sagent-tui --test blackbox_e2e -- --nocapture
cargo test -p sagent-runtime --test tool_actor approval_denial_persists_a_tool_error_without_starting_terminal -- --nocapture
cargo test -p sagent-tools --test terminal timeout_and_cancellation_terminate_the_process -- --nocapture
```

- TUI PTY 黑盒：4/4 通过，覆盖普通回合、terminal 审批、Ctrl-C 中断、RPC 断线重连。
- 审批拒绝：工具未执行，错误结果只持久化一次。
- terminal 超时/取消：子进程被终止，结果收口为单一终态。

### 质量门禁

```text
cargo run -p sagent-contracts
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
git diff --check
```

以上命令均通过；workspace 中仅显式 opt-in 的真实 Provider smoke 被忽略。

### 未覆盖与限制

- macOS/Linux 已按同一 P2.5 清单完成手工 smoke：启动/退出、启动失败恢复、Ctrl-C、
  审批拒绝、terminal 超时/取消和子进程清理均已验证。
- 真实 Provider smoke 仍保持显式 opt-in；默认验证不读取真实凭据，也不把真实服务加入 CI。
- 生产 panic 注入不作为默认 CI 步骤；启动失败恢复和 `TerminalGuard` 生命周期已覆盖
  常规终端恢复边界。

## macOS/Linux 完成记录

2026-09-13，已在 macOS 和 Linux 原生主机完成 P2.5 手工验证清单。两平台均确认真实
TUI/RPC 启停、失败后的终端恢复、Ctrl-C 中断、审批拒绝、terminal 超时/取消，以及
子进程树清理；结果与 Windows smoke 的行为契约一致。
