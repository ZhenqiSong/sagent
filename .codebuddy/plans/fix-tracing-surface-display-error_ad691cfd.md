---
name: fix-tracing-surface-display-error
overview: 修复 active_session.rs:181 的编译错误：tracing 日志中误用 Option<&str> 类型的 surface 参数，应改用已解包的 _surface（&str）。
todos:
  - id: fix-surface-log
    content: 将 active_session.rs 第 182 行 surface=%surface 改为 surface=%_surface
    status: completed
  - id: verify-check
    content: 运行 cargo check 验证编译通过
    status: completed
    dependencies:
      - fix-surface-log
---

## 用户需求

修复 `crates/sagent-cli/src/cli_core/active_session.rs` 第 181 行附近的编译错误。

## 核心问题

在 `try_acquire_active_session` 函数的 `tracing::info!` 日志宏中，`surface=%surface` 使用了 `Option<&str>` 类型的原始参数 `surface`，而 `%` 格式符要求值实现 `Display`，`Option<&str>` 未实现该 trait，导致 rustc 报错。

## 修复内容

将日志字段中的 `surface=%surface` 改为已解包的 `surface=%_surface`（`_surface` 为第 149 行 `surface.unwrap_or("cli")` 产生的 `&str`，已实现 `Display`），消除编译错误。

## 验证要求

`cargo check` 通过，不再出现 `Option<&str>` 未实现 `Display` 的报错。

## 技术栈

- 语言：Rust（Cargo Workspace）
- 日志：`tracing` + `tracing-subscriber`
- 涉及 crate：`sagent-cli`

## 问题定位

- 文件：`crates/sagent-cli/src/cli_core/active_session.rs`
- 函数：`try_acquire_active_session`（签名第 144-148 行，参数 `surface: Option<&str>`）
- 第 149 行已存在解包变量：`let _surface = surface.unwrap_or("cli");`（类型 `&str`）
- 报错位置：第 182 行 `tracing::info!(active=%active_count, max=%_max_sessions, surface=%surface, "达到最大会话限制")`

## 实现方案

Rust 的 `tracing` 宏中，`%` 格式符要求字段值实现 `Display`，`?` 要求实现 `Debug`。`Option<&str>` 仅实现 `Debug` 未实现 `Display`，因此不能使用 `%`。

最小且正确的修复是复用函数内已有的解包变量 `_surface`（类型 `&str`，已实现 `Display`），将 `surface=%surface` 改为 `surface=%_surface`。这样既消除报错，又保持了日志语义（记录解包后的表面来源，缺省为 "cli"）。

无需调整类型、新增依赖或改动其他调用点；`_surface` 在该函数作用域内已声明且被其他逻辑使用，改动零副作用。

## 执行细节

- 仅修改第 182 行一处字段名，保持其余日志字段（`active`、`max`）与消息字符串不变。
- 改动后运行 `cargo check` 验证编译通过，并确认无新增 warning 涉及该行。