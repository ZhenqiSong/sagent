# Sagent Rust 第四阶段 4.6 执行计划：交互 RPC、事件流与 Headless E2E

作者：SongZQ  
状态：执行中（4.5 已完成；4.6 步骤 0 已完成，尚未接入连接状态和 Runtime）  
前置条件：4.1～4.5 完成。

> 4.6 的目标是将 4.5 已有的 SessionActor、Provider、工具、审批和持久化事件安全公开为本地 NDJSON JSON-RPC 服务。交付物是可由真实子进程验证的 headless daemon；Ratatui UI 留给 4.7。

## 1. 完成后的用户可见闭环

```text
sagent-rpc --home <固定 Profile home>
  → gateway.ready
  → client.hello
  → session.create / session.resume
  → prompt.submit 立即返回 {status:"streaming", turn_id}
  → stdout 推送 delta、tool、approval、终态 event
  → session.interrupt 或 approval.respond
  → session.events.since 重放持久化事实
```

完成条件：

- stdout 仅含完整 NDJSON；日志、错误详情、backtrace 只写 stderr；
- response 与异步 event 只能经过一个 stdout writer，不能交错损坏一行 JSON；
- prompt、interrupt、approval 均通过 `SessionHandle` 投递给 Actor，RPC 不绕过 Actor/ToolWorker/ApprovalManager；
- `prompt.submit` 只等待 Actor 接收和 user message 持久化，不等待模型回复；
- 同一 session 忙时稳定返回 `session.busy`，第一版不做 prompt 队列；
- delta/usage 是瞬态 event；重连只从 SQLite daemon events 重放完成事实；
- streaming 期间仍可读取 interrupt 与 approval 请求；
- profile/home/DB path 在启动时固定，RPC params 不得覆盖它们。

## 2. 范围

### 本阶段实现

1. `client.hello` 的连接级状态和 capability gate；
2. `session.create`、`prompt.submit`、`session.interrupt`、`approval.respond`、`session.events.since`；
3. Profile-scoped Runtime bootstrap；
4. RuntimeEvent → JSON-RPC event 桥接；
5. 单 writer 异步 stdio transport；
6. Mock SSE 驱动的真实 `sagent-rpc` 子进程 E2E。

### 本阶段不实现

- Ratatui、raw mode、terminal guard、composer、approval overlay（4.7）；
- WebSocket/HTTP/桌面接入；
- 多 Provider fallback、MCP、browser、write_file、memory、cron；
- prompt FIFO、steer/redirect/branch/compression；
- 跨进程持久化 `ApprovalDecision::Always`；
- 重放 token delta，或重启后自动续跑 Provider/工具。

## 3. Python 参考行为

| Python 参考 | 保留的行为 | Rust 落点 |
| --- | --- | --- |
| `tui_gateway/server.py` 的 `write_json()`、`_event_frame()` | 单 transport writer；统一 event envelope；stdout 不能被日志污染 | outbound channel + writer task |
| `tui_gateway/server.py` 的慢 handler 分流 | 不能让流式回合阻塞 stdin，approval/interrupt 必须可及时读到 | tokio reader、dispatcher、event forwarder 并发 |
| `tui_gateway/methods_prompt.py` 的 `prompt.submit` | claim busy 后启动后台回合，立即回复 `streaming` | `SessionHandle::submit` |
| `tui_gateway/methods_session.py` 的 `session.interrupt` | 响应表示已受理，实际完成由 event 宣布 | `SessionHandle::interrupt` |
| `tui_gateway/methods_prompt.py` 的 `approval.respond` | 仅匹配当前 session/turn 的 pending request | `SessionHandle::resolve_approval` |
| `tui_gateway/event_replay.py` | 按 session sequence 补回缺失事件 | `Store::events_since` |
| `tui_gateway/entry.py` 的 `gateway.ready` | client 在第一条请求前知道协议边界 | ready 仅公告真实已启用 capability |

不复制 Python 的 `_sessions` 全局字典、ContextVar transport、线程池或内存 replay ring。Rust 直接使用 SessionSupervisor、每 session Actor mailbox 和 SQLite daemon events。

## 4. 当前 Rust 基线与关键修正

已有：

- `SessionHandle::{submit, interrupt, resolve_approval, resume, subscribe}`；
- `RuntimeEventKind` 覆盖文本流、工具、审批和终态；
- `Store::events_since` 按 `SessionId + EventSequence` 查询持久化事实；
- `resolve_openai_provider` 从固定 Profile config/.env 创建 Provider，密钥不序列化；
- `ClientHelloParams`、`ClientHelloResult`、稳定握手错误码和 `client.hello` 分发；
- `sagent-rpc` 仍为同步、只读的 stdin → dispatch → stdout 循环。

已在步骤 0 修正：hello DTO 不再公告尚未接通的交互 feature。feature 由真实注册表生成；在 handler 未完成前，ready/hello 仅公告只读方法和 `client.hello`。连接仍未保存握手结果，属于下一步骤的工作。

## 5. 目标结构

```text
stdin reader task
  → RpcConnection { ConnectionState, RpcRuntime, subscriptions }
  → request dispatcher
      ├─ readonly service
      └─ interactive runtime service
  → outbound mpsc<OutboundFrame>
  → stdout writer task（唯一写 stdout）

SessionHandle::subscribe()
  → event-forwarder task
  → outbound mpsc
```

`ConnectionState` 只存活于一条 stdio 连接：

- `client.hello` 成功后保存 `ClientCapabilities`；失败不会覆盖先前成功握手；
- 三阶段只读方法无需握手；
- create/submit/interrupt/events.since 必须 hello；
- approval.respond 还要求 `interactive_approval=true`；
- EOF、writer 失败或连接关闭时取消所有 event-forwarder；不自动 interrupt 已运行 Turn。

## 6. 协议契约

| 方法 | 参数 | 成功结果 | 前置条件 |
| --- | --- | --- | --- |
| `client.hello` | protocol version、client id、surface、capabilities | 协商版本、实际 features、busy policy | 无 |
| `session.create` | 可选 title | session id、摘要 | hello |
| `prompt.submit` | session id、非空 text、request id | streaming、turn id | hello |
| `session.interrupt` | session id、request id | interrupted 已受理 | hello |
| `approval.respond` | session id、turn id、approval id、decision | accepted | hello + interactive approval |
| `session.events.since` | session id、after sequence、limit | ordered events、latest sequence、分页状态 | hello |

DTO 全部 `#[serde(deny_unknown_fields)]`，params 只能是 JSON object，ID 使用强类型。请求不得传 home、profile、model override、endpoint、API key 或任意 workspace path。

错误保留 JSON-RPC 标准码，并使用稳定领域错误：

| 错误 | 场景 | data |
| --- | --- | --- |
| `handshake_required` | hello 前调用交互方法 | method |
| `unsupported_protocol_version` | version 不匹配 | requested、supported |
| `capability_not_granted` | 无审批能力调用 approval | capability、method |
| `session_not_found` | 当前 Profile 没有会话 | session id |
| `session_busy` | 活跃 Turn 拒绝第二个 prompt | session id |
| `no_active_turn` | interrupt/approval 无活动目标 | session/turn id |
| `runtime_unavailable` | bootstrap/provider/workspace 配置失败 | 不含 secret、路径、backtrace |

## 7. RuntimeEvent 映射

| RuntimeEventKind | JSON-RPC event | 回放 |
| --- | --- | --- |
| `PromptAccepted` | `turn.started` | 是 |
| `UserMessagePersisted` / `FinalMessagePersisted` | `message.committed` / `message.complete` | 是 |
| `ModelTextDelta` | `message.delta` | 否，瞬态 |
| `ModelUsage` | `usage` | 否，瞬态 |
| `ToolCallRequested` / `ToolStarted` / `ToolCompleted` | `tool.requested` / `tool.started` / `tool.completed` | completed 是 |
| `ApprovalRequested` | `approval.request` | 是 |
| `ApprovalResolved` / `ApprovalTimedOut` | `approval.resolved` / `approval.timed_out` | 是 |
| `TurnCompleted` / `TurnInterrupted` / `TurnFailed` | 同名 turn event | 是 |

实时 event 含 session id、可选 turn id；持久化重放额外含 SQLite sequence。不能给 delta 人工伪造 sequence。

## 8. 详细实施步骤

### 步骤 0：协议收口与 DTO（已有 hello 基础）

状态：已完成。

**位置**：`sagent-protocol/src/method/{client,session,prompt,approval}.rs`、`dispatch.rs`、`error.rs`。

1. 新建 prompt 与 approval 方法模块，定义全部 request/result DTO；
2. feature 列表由一个真实方法注册表产生，不声明未接通方法；
3. 提取纯 `ConnectionAccess::require_hello` 与 `require_interactive_approval`；
4. 补充 `session_busy`、`no_active_turn`、`runtime_unavailable` 的安全错误映射；
5. 测试 JSON 反序列化、未知字段、错误 UUID、版本不匹配、feature 真相。

**验收**：协议 crate 不依赖 runtime/tokio；不存在“返回 feature 但方法未注册”。

完成记录：新增 `prompt.*`、`approval.*`、会话创建/中断/事件回放 DTO，加入唯一 feature 注册表和纯连接访问规则；`gateway.ready` 与 `client.hello` 只公告真实已分发的方法。协议/RPC/工作区测试与 clippy 均已通过。

### 步骤 1：连接状态和状态化分发

状态：连接状态、握手保存和 capability gate 已完成；Runtime capability 注入留待步骤 3。

**位置**：新增 `sagent-rpc/src/connection.rs`；兼容保留 protocol 的只读 `dispatch()`。

1. 创建 `ConnectionState { client: Option<ClientCapabilities> }`；
2. hello 成功才写 state；
3. 交互 dispatcher 接收 `&mut ConnectionState`，所有 gate 只在一个入口执行；
4. client capability 通过 `SessionHandle::resume(client.clone())` 写入对应 SessionActor；
5. 测试 hello 前/后、两个连接隔离、非交互 client 的 approval 拒绝、三阶段只读兼容。

**验收**：连接 A 的 capability 永不授权连接 B；业务 params 不能伪造 capability。

完成记录：新增 `sagent-rpc/src/connection.rs`，由每条 stdio 连接独占
`ConnectionState`；`client.hello` 仅在版本协商成功后保存 `ClientCapabilities`，
`prompt.submit`、`session.create`、`session.interrupt`、`session.events.since` 和
`approval.respond` 在状态化分发入口统一执行握手/capability 检查。保留 protocol 的无状态
`dispatch()` 以兼容第三阶段只读调用方，并增加真实 `sagent-rpc` 子进程测试覆盖错误 hello、
连接隔离、只读兼容和审批拒绝。由于当前 RPC 尚未接入 Runtime，`SessionHandle::resume`
的 capability 传递将在步骤 3 的 RuntimeBootstrap 中完成。

### 步骤 2：异步 stdio 和唯一 writer

状态：已完成。

**位置**：重构 `sagent-rpc/src/stdio.rs`、`main.rs`、Cargo 依赖。

1. 启用 tokio `rt-multi-thread`、`io-util`、`sync`、`time`；
2. 分离 reader、dispatcher、event-forwarder、writer；
3. response/event 序列化为完整 frame 后进入有界 outbound channel，只有 writer `write_all + newline + flush`；
4. response、approval、interrupt、终态不可丢；delta 可合并或在订阅滞后时发诊断，绝不无界积压；
5. EOF 取消 connection scope、有界等待 writer；stdout 写失败终止进程和 forwarder；
6. 保留 1 MiB frame 限制、parse error、notification 不响应。

**验收**：并发 delta/response 的每一行都能独立解析；慢 client 不阻塞 interrupt 输入。

完成记录：`sagent-rpc` 已改为 Tokio 异步入口，stdin reader、ConnectionState dispatcher
和 stdout writer 分别运行在独立 task；请求与 outbound 帧均通过有界 channel 传递，stdout
只由 writer task 写入。新增受限帧读取器，超出 1 MiB 后会继续消费到换行再处理下一帧；
EOF、task 错误和 writer 失败会按 connection scope 取消/收口。现有 ready、parse error、
notification、超大帧和多响应顺序测试已迁移到异步 transport 并通过工作区验证。Transient
帧分类已预留，实际 delta 合并/丢弃策略由后续 event-forwarder 接入时实现。

### 步骤 3：RuntimeBootstrap 与 session.create

状态：已完成；未配置 workspace 时工具保持关闭，Provider 缺失仅影响后续 prompt.submit。

**位置**：新增 `runtime_bootstrap.rs`、`service/runtime.rs`，调整 `args.rs`/`main.rs`。

1. 启动时仅一次解析 `--home`/`--profile`，固定 `SagentPaths`；
2. 用 `resolve_openai_provider` 构造 `Arc<dyn ModelProvider>`；key 只能留在 bootstrap 内存；
3. 从 Profile 明确配置解析 model、workspace root、terminal limits、approval timeout；workspace 不可用时关闭相应工具或返回配置错误，不能放宽路径策略；
4. `Store::open_readwrite(state.db)` factory 注入 `SessionSupervisor`，每 Actor 独占 Store；
5. 注入 ToolDispatcher、ToolWorker、tool round 与 approval timeout；
6. `session.create` 写 `NewSession`，source 为 `rpc`，创建空会话时不启动 Actor。

**验收**：临时 Profile + Mock SSE 可启动 binary；stdout/stderr 均不泄露 API key；Profile 之间不共享 DB。

完成记录：新增 `runtime_bootstrap.rs` 与 `service/runtime.rs`。启动时固定
`SagentPaths`，初始化当前 Profile 的可写 state.db，并为每个未来 SessionActor 注入独占
`Store::open_readwrite` factory；Provider resolver 成功时配置 `SessionSupervisor`，失败时保留
只读浏览和空会话创建，供步骤 4 返回稳定 `runtime_unavailable`。`session.create` 已加入真实
feature 注册表和状态化 dispatch，必须先 hello；它以 `source = "rpc"`、当前模型和 RFC 3339
时间写入空 sessions 行，但不会创建 Generation、Turn、消息、事件或 Actor。由于 workspace
runtime 配置尚未定义，bootstrap 不会猜测当前目录，read_file/terminal 保持关闭。真实子进程
测试覆盖首次 state.db 初始化、握手 gate 和空会话持久化/无消息契约。

### 步骤 4：prompt.submit 与 event bridge

**位置**：`method/prompt.rs`、runtime RPC service、新增 `event_bridge.rs`。

1. 校验非空 `UserInput`，`get_or_start(session)`；
2. 在 submit 前建立/复用该 session 的 `subscribe()` forwarder，避免遗漏 `PromptAccepted`/首个 delta；
3. submit 成功立即响应 `{status:"streaming", turn_id}`；后续只由 event bridge 输出；
4. RuntimeEvent 的关联字段完全来自 Runtime，不信任客户端传入；
5. Busy、MailboxFull、ActorStopped、Persistence 映射稳定错误，不直接暴露 Rust Debug；
6. 同一 session 只允许一个 forwarder，actor/连接结束时清理。

**验收**：response 在首个 delta 前到达；`message.complete` 发生时 final message 已在 Store；同 session 第二个 prompt busy，其他 session 可并行。

### 步骤 5：interrupt 和 approval.respond

**位置**：`method/session.rs`、`method/approval.rs`、runtime service。

1. interrupt 调用 `SessionHandle::interrupt`；响应仅表示 Actor 接受，终态以 `turn.interrupted` event 宣布；
2. interrupt 要 hello，但不要求 approval capability；
3. approval 先 capability gate，再解析 `ApprovalDecision`，调用 `resolve_approval`；
4. handler 绝不直接执行 terminal 或操作 ApprovalManager；
5. 映射错误 session/turn、过期、重复、迟到 resolve；
6. 连接断开只停 event-forwarder，不隐式 interrupt Turn。

**验收**：危险 terminal 在批准前不启动；Once 仅恢复当前调用；Deny/timeout/late response 不执行命令；interrupt 会取消 provider、工具和 waiter。

### 步骤 6：持久化 event replay

**位置**：`method/session.rs`、Store DTO mapper、event bridge。

1. 复用 `Store::events_since`，limit 不超过 200；
2. 将 `StoredDaemonEvent` 映射为协议 event payload，不能嵌套完整 envelope 或 SQL row；
3. 返回升序 events、latest sequence、是否仍有下一页；
4. 客户端先 `session.resume` 获得 transcript，再按 sequence 补完成事实；
5. 不回放 delta、usage、spinner；
6. sequence 全局递增但按 session 查询，不能假设本 session 连号。

**验收**：重连后 tool/approval/终态顺序正确，其他 session 不串入。

### 步骤 7：headless E2E 与故障测试

**位置**：扩展 `crates/sagent-rpc/tests/stdio_integration.rs`，按行为拆分测试文件。

必须覆盖：

1. ready → hello → create → submit → delta → complete；
2. hello 前拒绝交互、hello 后 capability 生效、旧只读方法兼容；
3. 两个 session 并行流式，同 session busy；
4. streaming 中 interrupt：确认 response + 唯一 interrupted、无 final assistant；
5. approval request、Once、Deny、timeout、迟到 resolve；
6. events.since 仅回放持久化事实、不回放 delta；
7. malformed JSON、超大帧、未知方法、EOF、并发多帧 stdout；
8. params 试图覆盖 home/profile/model/key/workspace 时被拒绝；
9. 重启恢复旧 Turn fail-closed，新 prompt 可开始。

所有自动 E2E 使用临时 home、fixture config/.env 与本地 Mock SSE，不使用真实 key；真实 Provider smoke 保持 opt-in。

## 9. 推荐提交边界与质量门禁

```text
1. protocol：DTO、错误、真实 feature 注册表
2. rpc：ConnectionState 与 capability gate
3. rpc：异步 stdio 单 writer（尚不接 Runtime）
4. rpc：RuntimeBootstrap + session.create
5. rpc：prompt.submit + event bridge
6. rpc：interrupt + approval.respond
7. rpc：event replay + headless E2E
```

每步执行：

```text
cargo fmt --all
cargo test -p sagent-protocol --quiet
cargo test -p sagent-rpc --quiet
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

步骤 7 再执行 `cargo test --workspace --quiet`。

4.6 的最终验收：不用启动 TUI，测试客户端即可经真实 `sagent-rpc` 子进程完成普通文本回合、工具审批回合、中断和持久化事件回放；stdout 始终是合法 NDJSON，外部副作用始终受 4.5 的 Actor/ToolWorker/ApprovalManager 控制。
