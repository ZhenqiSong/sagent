# Sagent Rust 第四阶段 4.7 计划：Ratatui 薄客户端

作者：SongZQ  
状态：实施计划  
前置条件：4.1～4.6 已完成；`sagent-rpc` 已经提供 `client.hello`、会话浏览/创建、提交、事件流、中断、审批与事件回放。

> 4.7 的目标不是把 Agent 逻辑搬进终端界面，而是实现一个只消费本地 NDJSON JSON-RPC 的 Ratatui 客户端。TUI 不依赖 Store、Provider 或 Tools crate，不读取 SQLite，也不直接执行工具。

## 1. 用户可见闭环

```text
sagent tui --home <profile-home>
  ↓ 启动受控子进程
sagent-rpc --home <profile-home>
  ↓ stdin/stdout NDJSON
RPC client reader / writer
  ↓ 统一 reducer
Ratatui ViewModel
  ↓
会话列表 → transcript → composer → streaming → interrupt / approval
```

最小交付后，用户可以：

1. 启动 TUI，并在状态栏看到 RPC 连接状态；
2. 浏览、选择或创建会话；
3. 恢复已持久化 transcript；
4. 输入多行消息并实时看到 `message.delta`；
5. 在活跃 Turn 中按 `Ctrl-C` 请求中断；
6. 收到工具审批时选择 Once / Session / Always / Deny；
7. RPC 断线后重新握手、恢复 transcript，并通过 `session.events.since` 补读持久化事实。

## 2. 不可破坏的边界

| 边界 | 规则 |
| --- | --- |
| 薄客户端 | `sagent-tui` 只能依赖 `sagent-protocol`、`sagent-types` 与通用终端/异步库；不得依赖 Store、Runtime、Provider、Tools。 |
| 单一状态入口 | 键盘、粘贴、窗口尺寸、RPC response、RPC event、连接断开都转换为 `AppAction`，只由 reducer 修改 `AppState`。 |
| 绘制无副作用 | `draw` 只读取不可变 ViewModel；绝不发送 RPC、写文件、读取 stdin 或执行数据库查询。 |
| 连接能力 | hello 中声明 `surface=tui` 和 `interactive_approval=true`；不能以环境变量推断 TUI 是否支持审批。 |
| 恢复事实优先 | delta 是瞬态渲染状态；重连后以 `session.resume` 的消息快照和 `events.since` 的持久化事件为真相。 |
| 终端恢复 | 正常退出、RPC 启动失败、渲染错误和 panic 都必须恢复 raw mode、mouse/paste mode 与 alternate screen。 |
| Profile 隔离 | `--home`、`--profile` 只传给 RPC 子进程启动参数，绝不进入任意 JSON-RPC params。 |

## 3. 新 crate、依赖和模块布局

### 3.1 新 crate

在 workspace 中创建 `crates/sagent-tui`：

```text
crates/sagent-tui/
├── Cargo.toml
└── src/
    ├── main.rs              # 只做参数解析与 run 错误写 stderr
    ├── args.rs              # --home / --profile / --rpc-bin
    ├── terminal.rs          # TerminalGuard、panic hook、draw 生命周期
    ├── rpc/
    │   ├── mod.rs
    │   ├── client.rs        # 子进程、NDJSON request id、response waiter
    │   └── codec.rs         # 单行 JSON 编解码与 stdout 协议错误
    ├── app/
    │   ├── mod.rs
    │   ├── state.rs         # 纯 AppState/ViewModel
    │   ├── action.rs        # 所有输入事件的统一动作
    │   └── reducer.rs       # AppAction × AppState 的确定性归约
    └── ui/
        ├── mod.rs
        ├── layout.rs        # 主布局与尺寸断点
        ├── transcript.rs    # 消息/stream 渲染
        ├── composer.rs      # 多行编辑器
        ├── sessions.rs      # picker overlay
        └── approval.rs      # approval overlay
```

依赖建议：

```toml
ratatui = "0.30"
crossterm = { version = "0.29", features = ["event-stream"] }
tokio = { version = "1", features = ["macros", "process", "io-util", "sync", "time", "rt-multi-thread"] }
tokio-util = { version = "0.7", features = ["rt"] }
futures-util = "0.3"
unicode-width = "0.2"
```

版本最终以 lockfile 与三平台构建验证为准。`sagent-tui` 不增加新的持久化或 provider 依赖。

### 3.2 状态模型

```rust
struct AppState {
    connection: ConnectionView,
    profile_label: String,
    sessions: Vec<SessionSummaryDto>,
    active_session: Option<SessionView>,
    composer: ComposerState,
    overlay: Overlay,
    pending: PendingRequests,
    status: StatusLine,
}
```

`SessionView` 仅含可显示消息、当前 Turn、临时 delta buffer 与最后持久化 sequence：

```text
持久化 user/assistant/tool message → transcript 条目
message.delta                       → 当前 assistant 临时 buffer
message.complete                    → 清除临时 buffer，等待/应用最终事实
turn.completed/interrupted/failed   → 更新 active turn 与状态栏
```

不得从 delta 拼接并写入本地文件；最终内容必须来自后续 resume 快照或 RPC 明确的完成事实。

## 4. RPC client 合约

### 4.1 子进程所有权

`RpcClient::spawn` 启动：

```text
sagent-rpc [--home PATH] [--profile NAME]
```

- stdout 只能由 RPC reader task 读取；stderr 由独立诊断 task 有界收集，不能混进协议流；
- stdin 只能由 RPC writer task 写入，所有 request 先进入有界 outbound channel；
- request id 单调递增；`id → oneshot responder` 映射仅由 client task 管理；
- EOF、进程退出、非法 NDJSON、未知 response id 都转换为 `AppAction::RpcDisconnected`；
- TUI 退出先取消 reader/writer，再关闭 stdin，最后限时等待子进程；超时后才 kill。

### 4.2 启动握手顺序

```text
gateway.ready
→ client.hello(surface=tui, interactive_approval=true)
→ session.list
→ 自动 resume 上次选择的会话；没有时显示 picker
```

`gateway.ready` 缺失、版本不兼容或 hello 返回错误时不进入正常交互界面；状态栏显示可理解错误，并允许重试/退出。

### 4.3 response 与 event 路由

```text
带 id response → PendingRequests 对应的 AppAction
method=event   → 按 params.type 转为 RpcEvent Action
```

收到 event 时必须用 `session_id` 和 `turn_id` 关联目标；不是当前会话的事件不得写入当前 transcript，但可更新后台会话摘要的活动标记。

## 5. 分步实施

### 步骤 0：crate 骨架与编译边界

1. 创建 `sagent-tui` binary crate，并加入 workspace；
2. 只引入 `sagent-protocol`、`sagent-types`，在 CI 中断言不依赖 Store/Provider/Tools；
3. 创建 `AppState`、`AppAction`、`reduce` 的空骨架；
4. 创建 `sagent tui` CLI 入口或先提供独立 `sagent-tui` binary；首步优先独立 binary，CLI 子命令留到稳定后接入；
5. 添加 reducer 纯单元测试，证明 draw 不需要启动 terminal 或 RPC。

验收：`cargo run -p sagent-tui -- --help` 可运行；crate 依赖方向正确。

### 步骤 1：TerminalGuard 与主事件循环

参考 Python：`D:\projects\hermes-agent\ui-tui/` 的 terminal lifecycle、退出处理和输入循环；保留“始终恢复终端”的行为，不复制 React/Ink 组件。

1. 实现 `TerminalGuard::enter`：raw mode、alternate screen、cursor hidden、bracketed paste；
2. `Drop` 中按逆序恢复终端；恢复失败只写 stderr；
3. 安装 panic hook：先恢复 guard，再调用原 hook；
4. 建立 `tokio::select!` loop，接收 Crossterm event、RPC action、tick、resize；
5. draw 使用固定空状态栏和退出提示；`q`/`Ctrl-C` 在无活跃 Turn 时退出。

测试：guard 的副作用用可注入 terminal backend 测试；Windows/macOS/Linux 用手动 smoke 验证 panic 后终端可用。

完成记录：已引入 `ratatui`、`crossterm` 与为后续异步 client 预留的 `tokio`，实现
`TerminalGuard`。它在进入时启用 raw mode、alternate screen、bracketed paste 和隐藏光标，
在 Drop、部分初始化失败与 panic hook 中均尽力按反序恢复终端。空屏 loop 每 100ms 轮询
Crossterm，`q` 与首次 `Ctrl-C` 被规范化为 reducer 的 `QuitRequested`；draw 仅接收不可变
AppState。键盘映射通过纯单元测试覆盖；三平台的实际 terminal 恢复仍须在后续手动 smoke 中验证。

### 步骤 2：RPC 子进程 client 与握手

1. 实现 NDJSON codec：一行一个 JSON，对超长/非法行返回连接错误；
2. 实现 writer、reader、pending-response table 与 connection cancellation scope；
3. 启动后等待 `gateway.ready`，发送 `client.hello`；
4. 将协商 feature、连接状态、RPC stderr 摘要显示在状态栏；
5. 进程退出或 EOF 时取消所有 pending waiter，进入 `Disconnected { retry_at }`。

测试：使用 test child binary/fake stdio 验证 response 按 id 匹配、event 不会占用 response waiter、stdout 非 JSON 导致有界失败。

完成记录：已新增 `rpc::codec` 与 `rpc::client`。`RpcClient::spawn` 仅将 `--home`、
`--profile` 作为 `sagent-rpc` 的受控 argv，并分别独占子进程 stdin/stdout；NDJSON reader
限制单帧最大 1 MiB，非法 JSON、EOF、未知或非数字 response id 都会断开连接并取消全部
pending waiter。启动严格等待 `gateway.ready` 后再发送 `client.hello(surface=tui,
interactive_approval=true)`；协商成功才使 reducer 进入 `Connected`。已用纯 transport 与
response 路由测试覆盖单行编码、event/response 区分、超长帧和乱序 response。下一步接入
session picker 前，实际终端可使用 `--rpc-bin` 指向本地 `sagent-rpc` 二进制进行手动握手 smoke。

### 步骤 3：session picker、create 与 resume

1. hello 成功后发送 `session.list`；
2. 实现 session picker overlay：方向键、Enter、Esc、新建会话；
3. `session.create` 成功后立即 `session.resume`；
4. 将 `SessionResumeResult.messages` 映射为 transcript ViewModel；
5. 不访问 SQLite，不从 UI 猜测消息可见性或压缩规则。

测试：reducer 验证选择/新建/错误回滚；RPC fake 验证 create 后 resume 使用服务端返回的 session_id。

完成记录：已为 `AppState` 增加仅供绘制的会话摘要、活动会话快照、transcript 和 picker
overlay；新增 `app::controller`，它是唯一将 picker action 转换成 `session.list`、
`session.create`、`session.resume` 调用的副作用层。新建路径严格使用服务端返回的
`session_id` 再恢复；resume 的 messages 只映射为 ViewModel，TUI 不访问 SQLite 或重写
可见性规则。Ratatui picker 支持方向键/j/k、Enter、n 与 Esc；测试覆盖空列表边界、
不可信选择索引和服务端消息顺序映射。

### 步骤 4：composer、submit 与 streaming transcript

1. 实现多行 composer：字符输入、光标移动、Backspace/Delete、Enter 插入换行、`Ctrl-Enter` 提交；
2. bracketed paste 必须作为一次编辑动作插入，不逐字符触发 redraw/request；
3. composer 非空且当前会话无活跃 Turn 时才允许 submit；
4. `prompt.submit` 成功后记录 `turn_id`，清空 composer，并创建临时 assistant stream buffer；
5. `message.delta` 仅追加到匹配 session + turn 的临时 buffer；
6. `message.complete` / Turn 终态后触发轻量 `session.resume`，以数据库 transcript 替换临时显示。

测试：response 先于 delta、旧 turn delta 被忽略、同 session busy 错误不丢失 composer、CJK/emoji 宽度与长行换行稳定。

完成记录：已增加 Unicode 字符索引 composer、`Ctrl-Enter` 提交、Enter 换行与一次性
bracketed paste action；`prompt.submit` 成功后保存服务端 `turn_id` 并创建瞬态 stream buffer。
终端 tick 会消费 RPC event，只有 session_id 与 turn_id 同时匹配时才追加 `message.delta`；
`message.complete` 或 Turn 终态触发 `session.resume`，成功快照会替换 transcript 并清除
临时 turn，避免把 delta 当作持久化消息或重复恢复。测试覆盖 Unicode 删除与旧 Turn delta
忽略；真实 Provider streaming smoke 仍属于步骤 8 的 opt-in 验证。

### 步骤 5：interrupt、工具活动与 approval overlay

1. 有 active Turn 时 `Ctrl-C` 发送 `session.interrupt`；重复按键在 pending interrupt 期间去重；
2. 状态栏显示 tool.started/tool.completed 和等待审批状态，但不自行执行工具；
3. `approval.requested` 打开 modal，展示 Runtime 已脱敏 summary；
4. 映射按键：`1/2/3/0` 或明确菜单对应 Once / Session / Always / Deny；
5. `approval.respond` 成功后关闭 modal，等待后续 event；错误保留 overlay 并显示稳定错误；
6. `approval.timed_out`、`turn.interrupted`、`turn.failed` 必须关闭关联 overlay。

测试：没有 approval capability 时不发送 approval.respond；Deny 只发 DTO 不执行工具；interrupt response 不等于 Turn 已结束。

完成记录：已增加 active Turn 的 interrupt 去重标志、工具活动状态和 approval overlay。
有活跃 Turn 时 Ctrl-C 只发送一次 `session.interrupt`，其成功响应不会提前清除 Turn；只有
终态 event 才触发 resume。`approval.requested` 只消费 Runtime 的脱敏摘要，弹层将
`1/2/3/0/Esc` 映射为 Once/Session/Always/Deny 的 `approval.respond` DTO，TUI 不执行工具。
审批超时、审批已解决和匹配的 Turn 终态都会关闭弹层；提交失败保留弹层并显示错误。

### 步骤 6：恢复、重连与 ViewModel 一致性

1. 记录当前 session 的最后持久化 `sequence`；
2. RPC 断开后指数退避重启子进程，重新 hello；
3. 调用 `session.resume` 获取可见 transcript；
4. 调用 `session.events.since(after_sequence=checkpoint)` 补回 tool/approval/turn 终态；
5. delta buffer 在断线时丢弃，不能当成持久化消息；
6. 重连成功后恢复 active session；失败时保留只读错误屏和手动重试。

测试：模拟 EOF 后收到不连续 sequence；确认另一个 session 的 event 不进入当前 ViewModel；确认重连不会重复 transcript 条目。

完成记录：终端收到 transport 断线后会返回保留 ViewModel 的 `TerminalExit::Reconnect`；
入口按有界指数退避关闭旧 client、重启同一 Profile 的 RPC 子进程并重新 hello。恢复时先
用 `session.resume` 替换 transcript，再按当前会话的 checkpoint 分页调用
`session.events.since`；只接受严格大于 checkpoint 的同会话 sequence，并更新 checkpoint。
临时 delta 和 active Turn 不参与回放，避免把未持久化文本伪造成历史。

### 步骤 7：渲染质量与可访问性

1. 窄终端断点：隐藏侧栏，不截断 composer；
2. 使用 `unicode-width` 测量 CJK、emoji、组合字符，避免基于 Rust byte length 对齐；
3. transcript 虚拟窗口或有界缓存，长会话不导致每 tick 全量布局；
4. 明确 focus、键盘帮助、错误信息与 loading 状态；
5. 不使用颜色作为 approval/error 的唯一表达方式。

测试：纯 layout 测试覆盖 40/80/160 列、超长 token、CJK/emoji；snapshot 只测试 ViewModel/布局关系，不冻结整张 ANSI 屏幕截图。

完成记录：已引入 `unicode-width` 并实现按终端显示列宽的安全换行，中文、emoji 与长
无空格 token 不会按 UTF-8 byte 截断。布局在窄于 40 列时压缩状态栏与快捷键提示，但
始终保留 composer；transcript 每次仅拼接末尾 100 条，避免长会话使每个 redraw 全量
复制历史。测试覆盖窄布局断点、显式换行、CJK/emoji 和长 token 的列宽边界。

### 步骤 8：端到端与手动验证

1. 使用已有 `MockSseServer` 配置临时 Profile，启动真实 `sagent-tui` 与 `sagent-rpc`；
2. 自动验证 hello、session 创建、submit、delta、完成、数据库恢复；
3. 增加 interrupt、approval deny、断线重连的伪终端测试；
4. Windows/macOS/Linux 手动检查 raw mode、alternate screen 和 Ctrl-C 恢复；
5. 真实 Provider smoke 保持 opt-in，绝不进入默认测试。

完成记录：默认测试已覆盖重连退避的指数增长与 30 秒上限，以及 stdout 断线时必须丢弃
瞬态 active Turn/delta、同时保留 composer 的恢复契约。真实 Provider 与三平台 raw-mode
smoke 仍保持人工 opt-in 验证，不能写入默认 `cargo test`。

## 6. 初始不实现的项目

- Desktop/Electron、WebSocket/HTTP transport；
- markdown 富渲染、图片预览、鼠标复杂选择；
- branch/retry/steer、上下文压缩 UI；
- 多 Provider 路由、memory、MCP、browser、cron；
- 在 TUI 内编辑 config、Profile 或 API key；
- 任意绕过 `sagent-rpc` 的 Store/Provider 直接调用。

## 7. 完成定义

4.7 完成时必须满足：

```text
TUI → hello → list/create/resume → submit → stream → complete
                         ↘ Ctrl-C → interrupt → terminal event
                         ↘ approval overlay → approval.respond → Runtime event
断线 → reconnect → resume + events.since → 一致 transcript
```

并且：

- TUI crate 未引入 Store、Provider、Tools 依赖；
- 所有 stdout 协议解析、连接和终端恢复错误都不会把终端留在 raw mode；
- session/turn/event 关联完全采用服务端字段；
- `cargo fmt --all`、`cargo test --workspace --quiet`、`cargo clippy --workspace --all-targets -- -D warnings`、`git diff --check` 全部通过。
