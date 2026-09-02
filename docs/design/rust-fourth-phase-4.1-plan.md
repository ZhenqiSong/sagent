# Sagent Rust 第四阶段 4.1 计划：纯 Agent 状态机与 PromptSnapshot

作者：SongZQ  
状态：实施计划  
所属阶段：[第四阶段：最小可用 Agent 与交互式 TUI 垂直切片](D:/projects/sagent/docs/design/rust-fourth-phase-plan.md)  
前置条件：第三阶段的 Profile、SQLite 会话读取、JSON-RPC 只读服务和黑盒测试已经完成。

## 1. 目标

4.1 只建立 Agent 的**纯领域层**。它不调用模型、不访问 SQLite、不创建 Tokio task、不渲染终端，也不改变现有 JSON-RPC 方法。

本步骤要回答四个确定的问题：

1. 一个会话当前是否可以接收某个命令？
2. 接收命令或内部事件后，回合应该迁移到哪个状态？
3. 哪些状态迁移必须拒绝？
4. 当前 prompt 与 tool schema 是否仍与该会话首次模型调用时的 cache 边界兼容？

完成后，后续 Runtime、Provider、Tools 和 TUI 都依赖同一份状态与不变量，而不是各自判断“当前是否忙碌”或“是否还能继续调用模型”。

## 2. 范围与非目标

### 本步骤实现

- 新建 `sagent-agent` crate；
- 强类型 `TurnId`、`ApprovalId`、`ClientId` 与客户端 capability DTO；
- 回合状态、命令、领域事件和错误类型；
- 表驱动的状态迁移 reducer；
- 稳定 `PromptSnapshot`、canonical JSON、SHA-256 hash；
- transcript 角色交替和 tool-call 归属校验；
- 单元测试与 property 风格不变量测试。

### 本步骤不实现

- Tokio、actor mailbox、取消 token、SessionSupervisor；这些属于 4.3；
- SQLite migration、turn 持久化、event log；这些属于 4.2；
- HTTP、SSE、provider credential、真实模型调用；这些属于 4.4；
- 工具执行、approval UI、terminal process；这些属于 4.5；
- `client.hello`、`prompt.submit`、`session.interrupt` RPC；这些属于 4.6；
- Ratatui、composer、streaming UI；这些属于 4.7。

因此，4.1 的所有测试都必须在没有数据库、没有网络、没有终端的情况下运行。

## 3. 参考与设计约束

本计划遵循 [Rust 重写架构](D:/projects/hermes-agent/docs/design/rust-rewrite-architecture.md) 中的约束：

- [回合顺序唯一、prompt 前缀稳定、先持久化后完成](D:/projects/hermes-agent/docs/design/rust-rewrite-architecture.md:27)；
- [SessionActor 命令与状态机](D:/projects/hermes-agent/docs/design/rust-rewrite-architecture.md:188)；
- [PromptSnapshot 与 cache 安全](D:/projects/hermes-agent/docs/design/rust-rewrite-architecture.md:244)；
- [最小 Agent 的第一步](D:/projects/hermes-agent/docs/design/rust-rewrite-architecture.md:852)。

Python 行为参考：

| Python 文件 | 参考行为 | 4.1 的落点 |
| --- | --- | --- |
| `run_agent.py` 的 `AIAgent.run_conversation()` | assistant/tool 交替、模型循环、中断检查 | 用显式状态和事件表达；不复制同步 while loop |
| `run_agent.py` 的 `_build_system_prompt_parts()`、`_build_system_prompt()` | system prompt 的确定性组成 | `SystemPromptParts` 与 stable renderer |
| `toolsets.py` | toolset 决定模型可见 schema | tool schema canonical JSON 与 hash |
| `model_tools.py` | tool call 和 tool result 的关联 | transcript 中 tool 必须匹配紧邻 assistant 的 call |
| `tui_gateway/server.py` | busy session、interrupt、approval 的生命周期 | `SessionCommand` 与 `TurnState`，实际 RPC/actor 后置 |

## 4. crate 与模块设计

### 4.1.1 Workspace 调整

在根 `Cargo.toml` 的 `workspace.members` 加入：

```toml
"crates/sagent-agent",
```

新增：

```text
crates/sagent-agent/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── command.rs
    ├── event.rs
    ├── prompt.rs
    ├── state.rs
    ├── transcript.rs
    └── transition.rs
```

### 4.1.2 初始依赖

```toml
[dependencies]
sagent-types = { path = "../sagent-types" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
sha2 = "0.10"
```

禁止加入：

```text
tokio
tokio-util
rusqlite
reqwest
ratatui
crossterm
sagent-store
sagent-protocol
```

说明：`sagent-agent` 是纯领域层。只要依赖任何 transport、数据库或 UI，它就会难以用表驱动测试，也会反向污染后续 Runtime 边界。

### 4.1.3 模块职责

| 模块 | 职责 | 不应包含 |
| --- | --- | --- |
| `command.rs` | 外部意图的领域命令 | tokio channel、RPC JSON |
| `event.rs` | 状态机内部事件与失败分类 | JSON-RPC event envelope |
| `state.rs` | `TurnState`、状态相关纯值对象 | 数据库状态、Mutex |
| `transition.rs` | 合法迁移表与 reducer | provider/tool 实现 |
| `prompt.rs` | PromptSnapshot、canonical JSON、hash | config 文件读取、模型 HTTP |
| `transcript.rs` | 角色顺序与 tool-call 归属校验 | SQL 查询、UI 渲染 |
| `lib.rs` | 受控公开导出 | 业务实现堆放 |

## 5. 第一步：补充跨 crate 强类型

`TurnId`、`ApprovalId`、`ClientId` 是 Runtime、Protocol、Store 与 TUI 都会使用的稳定边界，因此应定义在 `sagent-types`，而不是 `sagent-agent`。

### 5.1 新类型

```rust
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TurnId(Uuid);

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApprovalId(Uuid);

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientId(Uuid);
```

要求：

- 对外 JSON 是普通 UUID 字符串；
- 提供 `new()` 生成方法、`parse()` 校验方法、`as_uuid()` 或 `as_str()` 读取方法；
- 不允许把 `SessionId`、`TurnId`、`ApprovalId` 互相作为字符串误传；
- 现有 `SessionId` 与 `MessageId` 不在 4.1 改格式，避免无关 migration。

### 5.2 客户端 capability

```rust
pub enum ClientSurface {
    Cli,
    Tui,
    Desktop,
    Web,
    Channel,
    Api,
}

pub struct ClientCapabilities {
    pub client_id: ClientId,
    pub surface: ClientSurface,
    pub interactive_approval: bool,
    pub supports_stream_edits: bool,
    pub protocol_version: u32,
}
```

4.1 只定义类型。4.6 的 `client.hello` 才把它绑定到一个实际 client/session；不允许此时通过环境变量决定“是否是 TUI”。

## 6. 第二步：定义状态、命令和领域事件

### 6.1 `TurnState`

```rust
pub enum TurnState {
    Idle,
    Preparing,
    CallingModel,
    ExecutingTools,
    AwaitingApproval,
    Completing,
    Interrupted,
    Failed,
}
```

语义：

| 状态 | 含义 | 能否接受新的 `SubmitPrompt` |
| --- | --- | --- |
| `Idle` | 当前没有活跃回合 | 可以 |
| `Preparing` | 接受输入、构建/校验 snapshot | 不可以 |
| `CallingModel` | provider 正在流式输出或等待结果 | 不可以 |
| `ExecutingTools` | 正在调用一个或多个已批准工具 | 不可以 |
| `AwaitingApproval` | 正等待用户批准/拒绝 | 不可以 |
| `Completing` | 正持久化最终结果 | 不可以 |
| `Interrupted` | 已收到取消，正在收尾 | 不可以 |
| `Failed` | 已失败，正在持久化失败结果 | 不可以 |

`Interrupted`、`Failed` 不能在 reducer 内“自动消失”；Runtime 之后必须明确提交 outcome，再发送收尾事件回到 `Idle`。

### 6.2 `SessionCommand`

```rust
pub enum SessionCommand {
    SubmitPrompt {
        request_id: RequestId,
        input: UserInput,
    },
    Interrupt {
        request_id: RequestId,
    },
    ResolveApproval {
        approval_id: ApprovalId,
        approved: bool,
    },
    Resume {
        client: ClientCapabilities,
    },
    Close,
}

pub struct UserInput {
    pub text: String,
}
```

规则：

- `UserInput.text` 在构造时 trim 后不得为空；
- `SubmitPrompt` 在非 `Idle` 状态必须产生明确 `SessionBusy` 错误；第一版不做排队；
- `Interrupt` 在 `Idle` 中应返回可区分的 `NoActiveTurn`，而不是伪造成功；
- `ResolveApproval` 的 ID 必须由未来 Runtime 与当前待审批项匹配；4.1 只定义匹配错误；
- `Close` 不能跳过活跃回合的 interrupt/finalize 语义；实际关闭工作在 4.3。

### 6.3 `TurnEvent`

```rust
pub enum TurnEvent {
    PromptAccepted,
    UserMessagePersisted { message_id: MessageId },
    PromptSnapshotReady,
    ModelTextDelta { text: String },
    ToolCallRequested { tool_call_id: ToolCallId },
    ApprovalRequested { approval_id: ApprovalId },
    ApprovalResolved { approval_id: ApprovalId, approved: bool },
    ToolResultReady { tool_call_id: ToolCallId },
    FinalMessagePersisted { message_id: MessageId },
    OutcomePersisted,
    Interrupted,
    Failed { failure: TurnFailure },
}
```

这些是领域事件，不等于：

- SQLite `daemon_events` 行；4.2 再映射；
- JSON-RPC `event` 帧；4.6 再映射；
- TUI 的动画/状态更新；4.7 再映射。

## 7. 第三步：实现表驱动状态迁移

### 7.1 纯 reducer 接口

```rust
pub fn transition(
    state: TurnState,
    event: &TurnEvent,
) -> Result<TurnState, TransitionError>;
```

建议 `TransitionError` 保存可序列化、可测试的 discriminant：

```rust
pub enum TransitionError {
    InvalidTransition {
        state: TurnState,
        event_kind: TurnEventKind,
    },
}
```

避免把完整 message content、secret、provider 返回体放进错误中。

### 7.2 最小合法迁移表

| 当前状态 | 事件 | 下一个状态 | 说明 |
| --- | --- | --- | --- |
| `Idle` | `PromptAccepted` | `Preparing` | actor 已独占该回合 |
| `Preparing` | `UserMessagePersisted` | `Preparing` | 用户消息写入不代表可调用模型 |
| `Preparing` | `PromptSnapshotReady` | `CallingModel` | prompt/tool schema 已锁定 |
| `CallingModel` | `ModelTextDelta` | `CallingModel` | 临时流式事件不改变状态 |
| `CallingModel` | `ToolCallRequested` | `ExecutingTools` | 转交工具执行 |
| `CallingModel` | `FinalMessagePersisted` | `Completing` | final message 已写入，等待 turn outcome |
| `ExecutingTools` | `ApprovalRequested` | `AwaitingApproval` | 工具等待用户决定 |
| `ExecutingTools` | `ToolResultReady` | `CallingModel` | 工具结果回到模型回环 |
| `AwaitingApproval` | `ApprovalResolved(approved)` | `ExecutingTools` | 获准后开始工具 |
| `AwaitingApproval` | `ApprovalResolved(rejected)` | `Completing` | 拒绝结果要被持久化 |
| `Completing` | `OutcomePersisted` | `Idle` | 回合完整提交 |
| 活跃状态 | `Interrupted` | `Interrupted` | 先进入取消收尾状态 |
| `Interrupted` | `OutcomePersisted` | `Idle` | interrupted outcome 已持久化 |
| 活跃状态 | `Failed` | `Failed` | 先进入失败收尾状态 |
| `Failed` | `OutcomePersisted` | `Idle` | failure outcome 已持久化 |

“活跃状态”指 `Preparing`、`CallingModel`、`ExecutingTools`、`AwaitingApproval`、`Completing`。

### 7.3 必须拒绝的迁移

以下必须成为测试而非注释：

```text
Idle + ToolResultReady
Idle + FinalMessagePersisted
CallingModel + PromptAccepted
AwaitingApproval + ToolResultReady
ExecutingTools + FinalMessagePersisted
Interrupted + ModelTextDelta
Failed + ToolCallRequested
```

## 8. 第四步：实现 PromptSnapshot

### 8.1 数据结构

```rust
pub struct PromptSnapshot {
    pub system_prompt: Arc<str>,
    pub system_hash: [u8; 32],
    pub tool_schema_json: Arc<str>,
    pub tool_schema_hash: [u8; 32],
    pub model_id: String,
    pub profile_revision: String,
    pub generation: u64,
}
```

### 8.2 `SystemPromptParts`

先定义结构，不直接在几十处拼接字符串：

```rust
pub struct SystemPromptParts {
    pub identity: String,
    pub operating_rules: Vec<String>,
    pub workspace_rules: Vec<String>,
    pub profile_context: String,
}
```

渲染规则：

1. 固定 section 顺序：identity → operating rules → workspace rules → profile context；
2. 每个 section 使用固定标题、换行和空行；
3. 列表项目保留调用方明确给出的顺序；
4. 禁止插入当前时间、随机 UUID、临时路径、动态可变 banner；
5. 相同输入必须生成逐字节相同输出。

### 8.3 canonical tool schema

输入可为 `serde_json::Value`，但输出必须确定：

```rust
pub fn canonical_json(value: &Value) -> Result<String, CanonicalJsonError>;
```

算法：

1. object 的所有 key 按 Unicode code point 升序；
2. 递归 canonicalize object 与 array 内部 object；
3. array 保持原有顺序；工具列表本身必须由未来 registry 确定排序；
4. 使用紧凑 JSON 输出，不加入格式化空格；
5. 不接受 NaN、Infinity 或无法合法序列化的值。

随后：

```rust
pub fn sha256(bytes: impl AsRef<[u8]>) -> [u8; 32];
```

### 8.4 兼容性检查

```rust
pub fn verify_compatible(
    expected: &PromptSnapshot,
    current: &PromptSnapshot,
) -> Result<(), PromptTransitionRequired>;
```

比较项至少包括：

- `system_hash`；
- `tool_schema_hash`；
- `model_id`；
- `profile_revision`；
- `generation`。

任一项不同都必须返回显式原因，例如 `SystemPromptChanged`、`ToolSchemaChanged`。不得自动替换 snapshot、不得悄悄继续调用模型。

4.1 只做内存比较；4.2 才持久化 snapshot；4.3 才在每个实际回合开始前调用比较。

## 9. 第五步：transcript 不变量

在 `transcript.rs` 定义与数据库 DTO 解耦的简化消息：

```rust
pub enum TranscriptRole {
    System,
    User,
    Assistant,
    Tool,
    Summary,
}

pub struct TranscriptMessage {
    pub id: MessageId,
    pub role: TranscriptRole,
    pub tool_call_id: Option<ToolCallId>,
    pub declared_tool_calls: Vec<ToolCallId>,
}
```

提供：

```rust
pub fn validate_transcript(
    messages: &[TranscriptMessage],
) -> Result<(), TranscriptInvariantError>;
```

最低不变量：

- 不允许相邻两个 `User`；
- 不允许相邻两个普通 `Assistant`；
- `Tool` 必须紧跟声明对应 `ToolCallId` 的 assistant；
- 普通 assistant message 不能携带不匹配的 tool result；
- `System` 只能出现在允许的开头/summary generation 边界；
- `Summary` 只能经未来 compression generation 路径产生。

不要在此函数中“修复”不合法历史；它只能返回明确错误。修复、迁移和兼容策略属于 Store migration 的单独决定。

## 10. 测试计划

### 10.1 ID 与 capability

- ID 序列化为 UUID 字符串；
- `TurnId` 不能反序列化为 `ApprovalId`；
- 各 `ClientSurface` JSON 表示稳定；
- capability 不包含环境变量推断逻辑。

### 10.2 状态迁移表

采用 table-driven tests：

```rust
struct Case {
    name: &'static str,
    from: TurnState,
    event: TurnEvent,
    expected: Result<TurnState, TurnEventKind>,
}
```

覆盖：

- 上述全部合法迁移；
- 上述全部非法迁移；
- interrupt 从每个活跃状态进入 `Interrupted`；
- `OutcomePersisted` 后才回到 `Idle`；
- 空文本 `UserInput` 被拒绝；
- 非 Idle 的 `SubmitPrompt` 被拒绝。

### 10.3 PromptSnapshot

- 相同 `SystemPromptParts` 输出逐字节相同；
- tool schema object key 输入顺序不同，canonical JSON/hash 相同；
- array 顺序变化，hash 必须不同；
- system rule、模型、profile revision、generation 任一变化，compatibility check 失败；
- hash 输出固定为 32 字节，测试使用已知 SHA-256 向量。

### 10.4 Transcript

- `user → assistant` 合法；
- `user → user` 非法；
- `assistant → assistant` 非法；
- `assistant(declares call-a) → tool(call-a)` 合法；
- tool 无 call ID、call ID 不匹配或跨 assistant 归属均非法；
- system/summary 位置非法时拒绝。

### 10.5 质量门禁

```text
cargo fmt --check
cargo test -p sagent-types --offline
cargo test -p sagent-agent --offline
cargo clippy -p sagent-types -p sagent-agent --all-targets --offline -- -D warnings
```

完成 4.1 后再运行 workspace 全量测试，确保第三阶段协议与现有 CLI 没有回归。

## 11. 实施顺序与提交边界

建议拆为五个独立提交：

1. `feat(types): add turn approval and client identifiers`；
2. `feat(agent): add command event and turn state types`；
3. `feat(agent): add table-driven transition reducer`；
4. `feat(agent): add stable prompt snapshot and canonical schema hash`；
5. `feat(agent): validate transcript role and tool-call invariants`。

每个提交必须可编译、可测试。不要在 4.1 混入 schema migration、Tokio actor 或 provider HTTP；否则无法定位 prompt/cache 或状态机错误的来源。

## 12. 完成定义

4.1 完成必须同时满足：

- workspace 中存在 `sagent-agent`；
- `sagent-agent` 不依赖数据库、网络、异步 runtime 或 TUI；
- `TurnState`、`SessionCommand`、`TurnEvent` 与 transition reducer 有完整测试；
- 非法迁移、空输入、忙碌 submit 均返回类型化错误；
- `PromptSnapshot`、canonical JSON 与 SHA-256 hash 的输出确定；
- snapshot 不兼容时返回明确 transition-required 原因；
- transcript 角色与 tool-call 不变量可独立验证；
- 新增 ID/capability 类型保持稳定 JSON 形状；
- format、test、严格 clippy 通过。

完成后，下一步是 4.2：将 generation、turn 和持久化领域事件以**加性 migration**写入 `sagent-store`。
