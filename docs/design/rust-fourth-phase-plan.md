# Sagent Rust 第四阶段计划：最小可用 Agent 与交互式 TUI 垂直切片

作者：SongZQ  
状态：实施计划  
前置条件：第一至第三阶段已完成 `sagent-types`、`sagent-config`、`sagent-store`、`sagent-protocol`、`sagent-cli` 与只读 `sagent-rpc`。

## 1. 重新定义第四阶段

第四阶段不再只是实现“只读会话浏览器”。当前已有的 Rust 基础层已经能够读取 Profile、SQLite 会话和 JSON-RPC 快照；继续只做浏览 TUI，无法验证 Sagent 最关键的运行时约束：单会话写入、prompt cache 稳定、持久化、取消、模型流式输出与工具权限。

本阶段应交付一个**最小可用的交互闭环**：

```text
Ratatui TUI
    ↓ JSON-RPC（stdio）
sagent-rpc / 本地 daemon
    ↓
SessionSupervisor → SessionActor
    ↓                   ↘
PromptSnapshot      OpenAI-compatible Provider → 流式事件
    ↓                   ↘
SQLite transcript ← 最小工具集（read_file / terminal + approval）
```

用户在复制的、独立的 Profile home 中可以：

1. 在 Rust TUI 中选择或新建会话；
2. 输入一条消息；
3. 看到模型文本流式显示；
4. 使用 `Ctrl-C` 安全中断；
5. 对 terminal 请求做批准或拒绝；
6. 退出后再次恢复相同 transcript；
7. 由 Rust TUI 或既有 CLI 读取完成后的会话记录。

这个范围直接对应架构文档 [最小可用 Agent 与 Rust TUI](D:/projects/hermes-agent/docs/design/rust-rewrite-architecture.md:852) 与 [首个可交付里程碑](D:/projects/hermes-agent/docs/design/rust-rewrite-architecture.md:1143)。

## 2. 不可破坏的约束

以下约束来自 [Rust 重写架构](D:/projects/hermes-agent/docs/design/rust-rewrite-architecture.md)，每一项都必须有自动化测试，而不是只写在文档中。

| 约束 | 第四阶段的具体规则 |
| --- | --- |
| Prompt 前缀稳定 | 首次模型调用后，system prompt、canonical tool schema、模型和 session capability 的 hash 不得静默改变；变化时拒绝当前回合并要求显式 transition 或新会话。 |
| 单一写者 | 每个 session 只有一个 `SessionActor` 可以改变 turn 状态和写入 transcript；provider/tool task 只能向 actor 发事件。 |
| 先持久化后完成 | user 消息、assistant 最终消息、tool 最终结果在通知客户端 `message.complete` 前必须已经提交 SQLite。token delta 永不作为持久化真相。 |
| Profile 隔离 | Profile 在 RPC 启动时固定；同一 RPC 进程不接受请求参数中的任意 home、DB path 或 profile。 |
| 取消端到端 | `session.interrupt` 同时取消 provider stream、运行工具、approval waiter 和已排队 prompt，并持久化 interrupted outcome。 |
| 会话能力 | TUI 是否支持 approval 等能力通过 `client.hello` 的 session capability 协商，不使用进程环境变量判断 UI surface。 |
| 加性迁移 | 新增 turn/event/prompt snapshot 数据表或列必须可由旧版本安全忽略；第四阶段禁止 destructive migration。 |

## 3. 参考 Python 代码：保留行为，不复制实现

| Python 参考 | 应提取的行为 | Rust 落点 |
| --- | --- | --- |
| `D:\projects\hermes-agent\run_agent.py` 的 `AIAgent.run_conversation()` | 模型调用、tool-call 回环、最终回复与中断检查 | `sagent-agent` 的纯状态机 + `sagent-runtime` 的 actor 编排；不复制 Python 巨型类 |
| `D:\projects\hermes-agent\run_agent.py` 的 `_build_system_prompt_parts()`、`_build_system_prompt()` | system prompt 由稳定组成部分生成 | `PromptSnapshot`、stable renderer、hash 回归测试 |
| `D:\projects\hermes-agent\toolsets.py`、`model_tools.py` | toolset 解析、schema 与工具调用边界 | `sagent-tools` registry；tool schema canonicalize 后锁定在 session generation |
| `D:\projects\hermes-agent\hermes_state.py` | session/message 持久化、恢复和压缩后的历史规则 | 复用并扩展 `sagent-store`，不让 provider/TUI 直接写 SQL |
| `D:\projects\hermes-agent\tui_gateway\server.py:2400-2438` | JSON-RPC result/error、参数校验、方法分派 | 扩展 `sagent-protocol`，所有 transport 共用相同契约 |
| `D:\projects\hermes-agent\tui_gateway\server.py` 中 `prompt.submit`、`session.interrupt`、lease/queue 逻辑 | 忙碌会话不能并发追加；中断具有优先级 | actor mailbox、有界 FIFO、优先 interrupt 命令，而不是全局 Mutex |
| `D:\projects\hermes-agent\agent/transports/chat_completions.py` | OpenAI Chat Completions/SSE 流解析 | 一个 OpenAI-compatible provider adapter；provider 专属 JSON 不泄露到 agent |
| `D:\projects\hermes-agent\ui-tui/` | transcript、composer、stream、approval、session picker 的交互顺序 | Ratatui reducer + RPC event；不要求视觉或组件结构复制 |

Python 的多 provider、子代理、cron、memory、MCP、browser、消息平台、桌面/Web transport 都不属于第四阶段。

## 4. 新 crate 与依赖方向

第四阶段新增以下 crate；现有 `sagent-rpc` 从“只读查询进程”演进为最小本地 runtime server，但仍保留 stdio NDJSON 启动方式。

```text
crates/
├── sagent-agent/       # 纯领域状态机、PromptSnapshot、模型无关 turn 输入输出
├── sagent-provider/    # ModelProvider trait 与 OpenAI-compatible adapter
├── sagent-tools/       # 最小 registry、read_file、terminal、approval policy
├── sagent-runtime/     # SessionSupervisor、SessionActor、取消、任务监管
└── sagent-tui/         # ratatui 薄客户端，只消费 JSON-RPC
```

依赖必须保持单向：

```text
sagent-types  ← config / store / protocol
sagent-agent  ← provider / tools
sagent-runtime ← agent / provider / tools / store / config / protocol
sagent-rpc / sagent-cli / sagent-tui ← runtime / protocol
```

限制：

- `sagent-agent` 不依赖 `tokio`、`ratatui`、`rusqlite`、HTTP client 或 crossterm；
- `sagent-tui` 不依赖 `sagent-store`、`rusqlite`、provider 或 tool crate；
- `sagent-provider` 不直接修改 SQLite；
- 只有 `SessionActor` 通过 repository/store 写 session、turn、message 和 event；
- 所有 async task 必须有 owner：actor、task supervisor 或 provider/tool invocation；禁止无归属的 `tokio::spawn`。

建议依赖：

```toml
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time", "process", "io-util"] }
tokio-util = { version = "0.7", features = ["rt"] }
async-trait = "0.1"
reqwest = { version = "0.12", features = ["json", "stream", "rustls-tls"] }
futures-util = "0.3"
ratatui = "0.30"
crossterm = { version = "0.29", features = ["event-stream"] }
unicode-width = "0.2"
```

实施时以 lockfile 与三平台构建为准，不在未验证前锁死精确版本。

## 5. 领域与存储设计

### 5.1 `sagent-agent`：纯状态机

定义可测试的状态，不把网络、SQLite transaction 或终端 I/O 放进状态机：

```rust
enum TurnState {
    Idle,
    Preparing,
    CallingModel,
    ExecutingTools,
    AwaitingApproval,
    Completing,
    Interrupted,
    Failed,
}

enum SessionCommand {
    SubmitPrompt { request_id: RequestId, input: UserInput },
    Interrupt { request_id: RequestId },
    ResolveApproval { approval_id: ApprovalId, approved: bool },
    Resume { client: ClientCapabilities },
    Close,
}
```

必须实现表驱动 transition function：

```text
Idle + SubmitPrompt                  → Preparing
Preparing + PromptPersisted          → CallingModel
CallingModel + TextDelta             → CallingModel
CallingModel + ToolCallCompleted     → ExecutingTools
ExecutingTools + ToolResult          → CallingModel
任意活跃状态 + Interrupt             → Interrupted → Idle
AwaitingApproval + ApprovalResolved  → ExecutingTools 或 Completing
Completing + FinalMessagePersisted   → Idle
```

非法迁移必须返回领域错误，不能依靠 `panic!` 或调用方“自觉不触发”。

### 5.2 `PromptSnapshot` 与 cache 边界

首次调用模型前生成并持久化：

```rust
struct PromptSnapshot {
    system_prompt: Arc<str>,
    system_hash: [u8; 32],
    tool_schema_json: Arc<str>,
    tool_schema_hash: [u8; 32],
    model_id: String,
    profile_revision: String,
    generation: u64,
}
```

规则：

- system prompt 必须由确定性 parts renderer 生成；
- tool schema 先递归排序并 canonical JSON 序列化，再计算 hash；
- 每个普通 turn 开始前重新计算并比较 hash；不一致时返回 `session.requires_transition`；
- context compression 是唯一可替换历史前缀并增加 generation 的路径；
- 第四阶段不实现 compression，但必须预留 generation 和明确拒绝语义。

### 5.3 Store 的加性数据

现有 messages/session schema 保持兼容。新增 migration 至少应支持：

```text
session_generations
  session_id, generation, system_hash, tool_schema_hash,
  model_id, profile_revision, created_at

turns
  turn_id, session_id, generation, state, user_message_id,
  assistant_message_id, started_at, completed_at, outcome

daemon_events
  sequence, session_id, turn_id, event_type, payload_json, created_at
```

- `daemon_events` 只记录可恢复的领域事实：turn started/completed/interrupted、message committed、tool outcome、approval resolved；
- token delta、spinner、typing 与局部渲染事件只发给客户端，不写入 event log；
- 写 final assistant message 与 `turn.completed` 使用同一短事务；
- 模型请求和工具运行期间绝不持有数据库 transaction；
- 继续使用 Profile 目录下的 SQLite；任何新表均应有历史 fixture migration test。

## 6. Provider、工具与 approval 的最小范围

### 6.1 一个 OpenAI-compatible provider

先定义 provider-neutral trait：

```rust
#[async_trait]
trait ModelProvider: Send + Sync {
    async fn stream(
        &self,
        request: ProviderRequest,
        sink: &mut dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ProviderFinish, ProviderError>;
}
```

实现顺序：

1. 本地 mock SSE server：文本 delta、最终 stop、半包 JSON、EOF、429、5xx、取消；
2. OpenAI-compatible Chat Completions adapter：请求、SSE、文本、tool call、usage；
3. 仅从当前 Profile 的 config/secret resolver 读取 endpoint 和 credential；不得新增用户行为环境变量；
4. provider task 只向 actor 发送中性事件，绝不直接改 transcript 或 stdout；
5. cancel 后不伪造 assistant final message；actor 写入 interrupted outcome。

第一版只支持一个显式选定 provider/model。fallback、路由、跨 provider resume 留到后续阶段。

### 6.2 最小工具集

按此顺序实现，不跳步：

1. `read_file`：workspace root canonical path 检查、大小上限、二进制检测；
2. `terminal`：显式 timeout、输出上限、进程树取消；Windows 使用 Job Object，POSIX 使用 process group；
3. `approval`：terminal 的危险操作必须由 TUI capability 触发 `approval.request`，actor 进入 `AwaitingApproval`；
4. `write_file`：只有在前述行为稳定后才加入，采用临时文件 + 原子 rename + audit outcome。

每个 tool 在编写 handler 前必须定义：名称、canonical schema、permission、timeout、idempotency、模型可见错误格式和审计字段。

## 7. 协议与 `sagent-rpc` 扩展

第三阶段的 `gateway.ready`、`gateway.ping`、`session.list`、`session.resume` 保持兼容。第四阶段新增协议版本协商和交互方法。

### 7.1 handshake

TUI 启动后第一条 client request：

```json
{"jsonrpc":"2.0","id":1,"method":"client.hello","params":{
  "protocol_version":1,
  "client_id":"uuid-or-local-id",
  "surface":"tui",
  "capabilities":{"interactive_approval":true,"supports_stream_edits":false}
}}
```

服务端返回协商版本、允许 feature、session policy。未协商的客户端只能调用第三阶段只读方法；交互方法返回稳定的 capability/handshake 错误。

### 7.2 新增 request

```text
session.create
prompt.submit
session.interrupt
approval.respond
session.events.since
```

- `prompt.submit` 返回 `{"status":"streaming","turn_id":"..."}`，不能等模型完成；
- 同一 session 忙碌时按明确策略拒绝或进入有界 FIFO；第一版建议拒绝并返回 `session.busy`，避免无 UI 的队列语义；
- `session.interrupt` 是高优先级请求，完成后返回稳定结果，实际完成通过 event 宣布；
- `approval.respond` 只能匹配当前 session 中待处理的 approval ID；
- `session.events.since` 按持久化 sequence 重放完成事件，供重连恢复；token delta 不重放。

### 7.3 新增 event

统一使用既有：

```json
{"jsonrpc":"2.0","method":"event","params":{"type":"...","payload":{}}}
```

第一版事件：

```text
turn.started
message.delta
message.complete
tool.started
tool.complete
approval.request
turn.interrupted
turn.failed
session.state
```

每个可归属会话的事件必须含 `session_id`；持久化事件还包含单调 `sequence`，可归属回合的事件包含 `turn_id`。

## 8. `sagent-tui`：阶段末实现的薄客户端

TUI 必须在 runtime/protocol 可用后实现，而不是反向定义 Agent 行为。

### 8.1 架构

```text
Ratatui draw loop（只读 ViewModel）
  ├─ Crossterm input task
  ├─ RPC reader task
  ├─ local tick / resize / paste task
  └─ 单一 reducer：输入与 RPC event → AppState
```

使用 `ratatui + crossterm + tokio`。draw loop 不执行 I/O；RPC reader 不直接修改 UI 状态；所有 keyboard/RPC event 在 reducer 中归约，便于测试过期 response、disconnect 和 overlay transition。

### 8.2 初始功能

- transcript，含 streaming text delta 和长文本/CJK/emoji 宽度处理；
- 多行 composer、bracketed paste、history；
- session list/resume/create；
- Ctrl-C interrupt；
- tool activity pane；
- approval overlay；
- profile/model/连接状态/活动回合状态栏；
- daemon EOF 后重连和基于 sequence 的完成事件恢复。

TUI 不保有 model state、tool state 或 SQLite connection；它只保存 ViewModel 和本地输入草稿。

### 8.3 终端安全

`TerminalGuard` 负责 raw mode、alternate screen 与 panic hook 恢复。正常退出、RPC 启动失败、provider error、panic 均必须恢复用户终端。Windows、macOS、Linux 各做手动与 CI 验证。

## 9. 详细实施步骤

### 4.1 建立 `sagent-agent` 与纯状态机

1. 定义 `TurnId`、`ApprovalId`、`ClientId`、`ClientCapabilities` 等强类型；
2. 实现 `TurnState`、`SessionCommand`、`SessionEvent`、表驱动 transition reducer；
3. 实现 `PromptSnapshot`、canonical JSON 和 hash helpers；
4. 添加状态迁移、角色交替、schema 顺序稳定、prompt hash 不变的单元/property tests。

验收：不启动 tokio、SQLite 或 HTTP 也能完整测试 agent 状态机。

### 4.2 扩展 Store 与事件持久化

1. 添加 generation、turn、daemon event migration；
2. 实现 repository API：创建 turn、提交 user/final assistant/tool message、完成/中断 turn、按 sequence 查询 event；
3. 为历史 schema、失败事务、重复完成、Profile 隔离和 FTS 回归写测试；
4. 验证旧 CLI 仍能读出新写入的 session/message 基础字段。

验收：每个 completed/interrupted turn 有确定持久化结果；不产生半完成的 final message。

### 4.3 建立 `sagent-runtime` SessionActor

1. 创建 `SessionSupervisor`，以 `SessionId` 管理有界 actor mailbox；
2. Actor 成为唯一 DB 写入者；provider/tool worker 通过 channel 回传事件；
3. 实现 submit、busy 拒绝、interrupt、close 和 actor 生命周期；
4. 引入 task supervisor/cancellation token；
5. 对同 session 并发 submit、interrupt 竞态、actor crash、数据库失败写 integration tests。

验收：两个并发 prompt 不会产生相邻 user message，也不会并发写同一 transcript。

### 4.4 实现 mock + OpenAI-compatible provider

1. 先以 mock SSE 驱动 actor；
2. 接入真实 OpenAI-compatible endpoint；
3. 实现 delta、finish、usage、tool call、EOF、网络错误和 cancel 分类；
4. 建立 profile-scoped credential resolver；
5. 以固定 fixture 对照 Python transcript 顺序。

验收：一个普通文本回合可流式显示、完成持久化、退出后恢复；cancel 没有伪造最终 assistant 内容。

### 4.5 实现最小工具与 approval

1. `read_file`；
2. terminal process supervision 与 timeout/cancel；
3. approval state/event/RPC/TUI capability；
4. tool schema lock-in 与 prompt hash 防御检查；
5. tool result 持久化与恢复。

验收：未批准危险 terminal 操作不会执行；批准/拒绝/interrupt 都有确定结果和审计记录。

### 4.6 扩展 RPC 并完成 headless 端到端测试

1. 实现 `client.hello`、submit、interrupt、approval、event replay；
2. 将当前 `sagent-rpc` stdio 循环接到 runtime，保持第三阶段只读方法兼容；
3. 用真实子进程覆盖 ready、hello、submit、delta、complete、interrupt、approval、EOF、reconnect；
4. stdout 继续只能输出 NDJSON；日志与 backtrace 仅 stderr；
5. 对协议版本/feature 不兼容返回明确错误。

验收：不启动 TUI 也能以测试客户端完成完整交互回合。

### 4.7 实现 Ratatui TUI

1. 终端 guard、panic 恢复、连接启动和 reducer 骨架；
2. transcript + composer + streaming；
3. session picker/create/resume 与状态栏；
4. tool activity、approval、interrupt；
5. resize、CJK/emoji、paste、长 transcript、断线重连；
6. 真实 `sagent-rpc` 端到端终端测试和手动 smoke test。

验收：TUI 只是 runtime 的薄客户端；不引用 Store/Provider/Tools crate。

## 10. 测试与质量门禁

| 层级 | 必须覆盖 |
| --- | --- |
| unit | 状态迁移、PromptSnapshot hash、schema canonicalization、配置/权限解析 |
| property | role alternation、同输入 schema 确定性、Profile/session 隔离 |
| integration | SQLite migration/rollback、真实 FTS、mock SSE、tool timeout/cancel、actor mailbox |
| contract | Python/Rust 的 request/event/result、transcript 与 tool-schema fixture |
| E2E | stdio daemon、submit/stream/interrupt/approval/reconnect、Ratatui session 恢复 |
| manual | Windows/macOS/Linux raw mode 恢复、terminal process tree cancel、真实 provider smoke |

每次提交只迁移一个行为边界：例如“actor interrupt”或“provider SSE parser”，不能在同一改动中同时改变 schema、协议、provider 和 TUI。

运行门禁：

```text
cargo fmt --check
cargo test --workspace --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
```

并在 Windows、macOS、Linux CI 增加 contract/E2E 覆盖。

## 11. 完成定义

第四阶段完成须同时满足：

- 一个 Profile、一个 OpenAI-compatible provider 和最小工具集可形成完整回合；
- 每个 session 有唯一 Actor，submit、tool、approval、interrupt 均经过 actor；
- prompt prefix/tool schema 在普通回合中稳定，违反时有显式 transition 错误；
- user/final assistant/tool outcome 均先持久化，再发送完成事件；
- TUI 支持 composer、stream、interrupt、approval、session list/resume 和 reconnect；
- TUI 不直接读取 SQLite，也不拥有 model/runtime state；
- Rust 或既有客户端可在完成后读取同一个 transcript；
- migration 为加性，能在复制的 Profile home 上运行；
- workspace 门禁及三平台 CI 全部通过。

## 12. 延后到第五阶段的事项

- 多 provider 路由、fallback、credential pool；
- memory、MCP、browser、cron、delegation；
- WebSocket/HTTP daemon transport、Desktop/Web 接入；
- 完整插件 SDK/Python compatibility worker；
- context compression、branch/retry 的 runtime generation 实现；
- 消息 gateway 与各平台 adapter。

这些能力都依赖第四阶段的 actor、持久化 event 和协议契约稳定后再扩展，不能为了提前展示界面而跳过基础运行时。
