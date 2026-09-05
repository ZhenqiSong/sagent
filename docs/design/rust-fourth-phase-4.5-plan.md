# Sagent Rust 第四阶段 4.5 执行计划：最小工具、Terminal 与 Approval

作者：SongZQ  
状态：规划中  
前置条件：4.1、4.2、4.3、4.4 已完成。

> 本计划以当前 Rust 代码为基线，同时对照 Python Hermes 的实际行为。4.5 的目标不是一次性复制所有 Python 工具，而是先建立一个安全、可测试、可恢复的最小工具闭环。

## 1. 阶段目标

4.4 已经完成了：

```text
Prompt
  → Profile resolver
  → OpenAI-compatible Provider
  → SSE delta / finish
  → Provider worker
  → SessionActor
  → Store
  → assistant message
```

4.5 将回合扩展为：

```text
Provider tool call
  → Actor 校验 tool schema 和 ToolCallId
  → ToolRegistry 查找工具
  → permission/approval 判断
  → read_file 或 terminal worker
  → tool result
  → Store 持久化 assistant tool-call + tool message
  → 重新构造 PromptSnapshot
  → Provider 继续生成
  → final assistant message
```

本阶段完成后，模型可以在一个 Turn 中调用最小工具，并且：

- 工具调用与结果可以通过 `ToolCallId` 一一对应；
- 危险 terminal 命令在未审批前不会执行；
- 取消会同时停止 Provider、工具进程和 approval 等待；
- 工具结果会进入数据库并在会话恢复时重建上下文；
- 工具 schema 变化会被 generation/tool-schema hash 检测到；
- 工具 worker 不直接访问 Store，SessionActor 仍然是唯一写入者。

## 2. 与 4.6、4.7 的边界

### 2.1 4.5 实现

- `sagent-tools` registry 和 canonical schema；
- `read_file`；
- terminal process supervision；
- approval policy、pending approval 和超时；
- Provider tool-call 聚合；
- Actor 中的工具回合循环；
- tool-call、tool-result、approval outcome 持久化；
- RuntimeEvent 的工具和审批事件；
- 面向 RPC/TUI 的稳定 DTO 和 capability 判断；
- 完整的 Mock Provider、Mock Tool、Actor、Store 集成测试。

### 2.2 4.5 不实现

- 不实现所有 Python 工具；第一版只做 `read_file` 和 `terminal`；
- 不实现 `write_file`、patch、browser、web、MCP、delegate 等扩展工具；
- 不实现多 Provider fallback、自动重试和 credential pool；
- 不在工具中直接执行 SQL；
- 不让工具修改 SessionActor 内部状态；
- 不完成 stdio JSON-RPC 方法分派；公开 RPC 接线属于 4.6；
- 不实现 Ratatui UI，approval overlay 属于 4.7；
- 不默认启用 yolo 或任何绕过 approval 的行为。

`approval.respond` 的参数模型和事件 DTO 可以在 4.5 定义，但真正接入 `sagent-rpc` 的 dispatch、handshake 和客户端 UI 放在 4.6/4.7。

## 3. Python 参考代码

| Python 文件 | 需要重点阅读的内容 | Rust 计划落点 |
| --- | --- | --- |
| `D:\projects\hermes-agent\model_tools.py` | `handle_function_call`、tool schema 获取、工具调用参数清洗、pre/post tool hook、结果错误格式 | `sagent-tools::registry`、runtime tool dispatcher |
| `D:\projects\hermes-agent\toolsets.py` | toolset 定义、工具按能力/服务筛选、schema 暴露边界 | `ToolRegistry`、`ToolSchemaSet`、generation hash |
| `D:\projects\hermes-agent\tools\registry.py` | 注册、发现、schema、`check_fn` 和工具名称冲突处理 | Rust registry 注册与校验；不复制进程级缓存 |
| `D:\projects\hermes-agent\tools\file_tools.py` | `read_file` 参数、路径处理、文件大小、输出截断和错误格式 | `sagent-tools::read_file` |
| `D:\projects\hermes-agent\tools\path_security.py` | workspace/root 约束、路径规范化、符号链接与敏感路径防护 | `WorkspaceRoot`、canonical path policy |
| `D:\projects\hermes-agent\tools\terminal_tool.py` | terminal 参数、timeout、输出限制、危险命令审批和错误返回 | `sagent-tools::terminal`、approval policy |
| `D:\projects\hermes-agent\tools\approval.py` | `detect_dangerous_command`、session approval、pending approval、timeout、deny/allow | `ApprovalManager`、`ApprovalDecision`、`ApprovalRequested` |
| `D:\projects\hermes-agent\tools\environments\local.py` | 本地子进程环境、cwd、继承环境变量和平台差异 | `LocalProcessRunner` |
| `D:\projects\hermes-agent\tools\process_registry.py` | 进程跟踪、取消和清理 | `ToolProcessHandle`、进程树终止 |
| `D:\projects\hermes-agent\run_agent.py` | tool-call 回环、assistant tool-call 与 tool message 的顺序、取消传播 | `SessionActor` 的 tool loop |
| `D:\projects\hermes-agent\agent\tool_executor.py` | 顺序/并行调用、结果归并、异常收口 | 4.5 先顺序执行；并行策略留为后续扩展 |
| `D:\projects\hermes-agent\tui_gateway\server.py` | approval request/respond、session interrupt、事件发送和重连恢复 | RuntimeEvent/协议 DTO；实际 RPC dispatch 在 4.6 |
| `D:\projects\hermes-agent\hermes_state.py` | tool message、tool_call_id、短事务和 transcript 恢复 | `sagent-store` 工具消息事务 |

Python 只提供行为参考。Rust 不复制 Python 的全局字典、ContextVar、线程共享状态或同步阻塞等待；所有可变 approval/tool 状态必须归属 SessionActor 或明确的受监管 worker。

## 4. 当前 Rust 基线

### 4.1 已有类型

- `sagent_types::ToolCallId`；
- `sagent_types::ApprovalId`；
- `sagent_types::ClientCapabilities`；
- `sagent_agent::ApprovalDecision::{Once, Session, Always, Deny}`；
- `sagent_agent::SessionCommand::ResolveApproval`；
- `sagent_agent::PromptMessage.tool_calls`；
- `sagent_agent::Transcript` 的 pending/completed tool-call 校验；
- `sagent_provider::ProviderEvent::ToolCallDelta`；
- `sagent_store::Store::commit_tool_result`；
- `sagent_runtime::RuntimeEventKind` 和 `WorkerEvent`；
- `CancellationToken`、SessionActor 唯一写入边界。

### 4.2 当前缺口

- Provider worker 目前把 `ToolCallDelta` 转成“不支持工具调用”错误；
- 没有工具注册表和工具 schema canonicalization；
- 没有 workspace root/path security；
- 没有 terminal process runner；
- `ResolveApproval` 尚未由 Actor 处理；
- 没有 pending approval 生命周期；
- Store 只有 tool result 写入，还没有 assistant tool-call 消息的事务入口；
- Runtime 没有 tool.started、tool.completed、approval.request 等事件；
- PromptSnapshot 尚未根据工具结果重新构造并再次调用 Provider。

## 5. 目标依赖关系

```text
sagent-types
    ↑
sagent-agent ────────┐
                     ├── sagent-tools ──── sagent-runtime ─── sagent-rpc
sagent-store ────────┘          │                  │
                                └── tokio process  └── RuntimeEvent
```

约束：

1. `sagent-tools` 可以依赖 `sagent-agent`、`sagent-types`，不能依赖 `sagent-runtime`；
2. `sagent-tools` 的 handler 返回结构化结果，不写 Store、不发布 RuntimeEvent；
3. `sagent-runtime` 负责把工具结果转换为 Store 操作和 RuntimeEvent；
4. `sagent-store` 不依赖工具实现，只保存通用消息和事件；
5. `sagent-rpc` 只调用 Runtime 公开接口，不直接调用工具 handler；
6. API key、完整环境变量和进程私密输出不能进入 tool result、daemon event 或日志。

## 6. 核心领域模型

### 6.1 ToolDefinition

建议在 `sagent-tools/src/definition.rs` 定义：

```rust
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub permission: ToolPermission,
    pub timeout: Duration,
    pub output_limit: usize,
}
```

`ToolDefinition` 必须满足：

- name 非空且只包含稳定的工具名字符；
- schema 是 JSON object；
- schema canonical JSON 顺序稳定；
- 同名工具不能重复注册；
- timeout/output_limit 必须有上限；
- permission 不由模型参数决定；
- `Debug` 和错误信息不能包含 secret。

### 6.2 ToolCall

```rust
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    pub arguments: serde_json::Value,
}
```

解析要求：

- Provider 的多个 delta 必须按顺序合并；
- 缺少 id、name 或 JSON 参数非法时，整个 tool call 失败；
- 同一 Turn 内 ToolCallId 不得重复；
- 未注册工具不能执行；
- 参数必须通过工具 schema 校验后再进入 handler。

### 6.3 ToolResult

```rust
pub struct ToolResult {
    pub tool_call_id: ToolCallId,
    pub name: String,
    pub ok: bool,
    pub content: String,
    pub truncated: bool,
    pub exit_code: Option<i32>,
    pub error_kind: Option<String>,
}
```

工具结果必须是有限大小的文本。超出限制时保留明确的截断标记，不能无限累积内存。

### 6.4 ApprovalRequest

```rust
pub struct ApprovalRequest {
    pub approval_id: ApprovalId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub tool_call_id: ToolCallId,
    pub tool_name: String,
    pub summary: String,
    pub policy_key: String,
    pub expires_at: String,
}
```

展示给用户的 summary 必须经过脱敏；原始命令不能作为未经处理的 UI 文本或错误正文传播。

## 7. 状态与消息时序

### 7.1 正常工具回合

```text
1  Store::begin_turn(user)
2  ProviderRequest(tool schemas included)
3  Provider TextDelta（只广播）
4  Provider ToolCallDelta（worker 聚合）
5  Actor 校验并持久化 assistant(tool_calls)
6  ToolRegistry 查找工具
7  ToolStarted
8  Tool worker 返回 ToolResult
9  Store::commit_tool_result(tool message + tool.completed)
10 根据 user + assistant(tool_calls) + tool 结果构造新 PromptSnapshot
11 再次调用 Provider
12 Store::complete_turn(final assistant)
13 FinalMessagePersisted
14 TurnCompleted
```

### 7.2 需要 approval 的 terminal

```text
ToolCall
  → detect permission
  → ApprovalRequested 持久化/广播
  → TurnState::AwaitingApproval
  → ResolveApproval
      ├─ Once/Session/Always → RunningTool → 执行
      ├─ Deny                → ToolResult(error) 或 TurnFailed
      ├─ Timeout             → TurnFailed
      └─ Interrupt           → Interrupted，不启动进程
```

### 7.3 取消

```text
session.interrupt
  → Actor cancellation.cancel()
  → Provider 停止读取
  → Tool process kill tree
  → Approval waiter 返回 cancelled
  → Actor 只允许一个 Interrupted 终态
  → 不伪造 tool result，不伪造 final assistant
```

## 8. 详细执行步骤

### 步骤 0：建立行为清单与 fixture

#### 需要参考

- `model_tools.py:handle_function_call`；
- `tools/file_tools.py` 的 read 参数和错误；
- `tools/terminal_tool.py` 的 command/timeout/approval；
- `tools/approval.py` 的危险命令检测和 session approval；
- `run_agent.py:_execute_tool_calls*`；
- `hermes_state.py` 的 tool message 写入；
- 当前 `sagent-agent/src/transition.rs` 和 `sagent-store/src/turn.rs`。

#### 需要完成

1. 建立 Python 行为对照表：输入、校验、结果、错误、是否需要 approval；
2. 新增 `crates/sagent-tools/tests/fixtures/`；
3. 固定最小 read_file/terminal 请求与结果 JSON；
4. 固定危险命令、超时、输出超限、路径越界、approval timeout fixture；
5. 明确 4.5 不复制 Python 的 plugin hook 和多工具扩展点。

#### 验收

- 每个后续测试都有稳定 fixture；
- fixture 不包含真实路径、API key 或用户数据；
- tool result 与 Python transcript 的 role/tool_call_id 顺序一致。

### 步骤 1：创建 `sagent-tools` crate 与 registry

#### 创建文件

```text
crates/sagent-tools/
├── Cargo.toml
├── src/lib.rs
├── src/definition.rs
├── src/registry.rs
├── src/schema.rs
├── src/error.rs
└── tests/registry.rs
```

#### 实现内容

1. 在 workspace 注册 `sagent-tools`；
2. 实现 `ToolDefinition`、`ToolRegistry`、`ToolPermission`、`ToolResult`；
3. 提供 `register`、`get`、`names`、`definitions`；
4. 提供 `canonical_tool_schema` 和 `tool_schema_hash`；
5. 拒绝空名称、重复名称、非 object schema 和不稳定 schema；
6. registry 只保存 handler/definition，不保存 Session 或 Turn 状态；
7. 将 registry 生成的 schema 集合交给 `SessionActor` 创建 generation；
8. 工具 schema hash 变化时禁止复用旧 PromptSnapshot。

#### 测试

- 同一 registry 多次 canonicalize 得到相同 hash；
- 注册顺序变化不影响 hash；
- 重复工具名被拒绝；
- 未注册工具返回稳定错误；
- schema 不能包含秘密或运行时随机字段；
- registry 不依赖 SQLite。

### 步骤 2：实现 `read_file`

#### 参考代码

- `tools/file_tools.py`：读取参数、输出格式和异常；
- `tools/path_security.py`：路径规范化和 root containment；
- `tools/binary_extensions.py`：二进制文件判断；
- `tools/tool_output_limits.py`：输出上限和截断策略。

#### 创建文件

```text
crates/sagent-tools/src/workspace.rs
crates/sagent-tools/src/read_file.rs
crates/sagent-tools/tests/read_file.rs
```

#### 实现规则

1. `WorkspaceRoot` 从显式 Profile/workspace 配置构造；
2. 相对路径相对于 workspace root 解析；
3. 使用 canonical path 检查路径必须位于 root 内；
4. 拒绝 `..` 越界、绝对路径逃逸和不允许的符号链接；
5. 检查文件是否存在、是否普通文件、是否可读取；
6. 二进制文件返回明确的 `binary_file` 错误，不把原始 bytes 塞进模型上下文；
7. 设置最大文件字节数和最大输出字符数；
8. 超限返回截断结果和 `truncated=true`，不能静默丢失状态；
9. 读取期间响应 `CancellationToken`；
10. 不执行 shell，不访问 Store，不写审计事件。

#### 测试

- 正常 UTF-8 文本；
- CJK/emoji；
- 路径越界；
- symlink 逃逸；
- 目录而非文件；
- 二进制文件；
- 文件超限；
- 读取取消；
- workspace root 不同 Session 之间隔离。

### 步骤 3：实现 terminal process supervision

#### 参考代码

- `tools/terminal_tool.py`：参数、timeout、危险命令和结果格式；
- `tools/environments/local.py`：本地环境、cwd、环境变量；
- `tools/process_registry.py`：进程生命周期和清理；
- `tools/approval.py:detect_dangerous_command`：approval 前置判断。

#### 创建文件

```text
crates/sagent-tools/src/process.rs
crates/sagent-tools/src/terminal.rs
crates/sagent-tools/src/command_policy.rs
crates/sagent-tools/tests/terminal.rs
```

#### 实现规则

1. 定义 `TerminalRequest { command, cwd, timeout, output_limit }`；
2. cwd 必须通过 `WorkspaceRoot` 校验；
3. 明确 shell 策略，不允许模型通过参数绕过 cwd/root 限制；
4. 默认 timeout 和最大 timeout 都由配置提供，不能无限等待；
5. stdout/stderr 分开读取并统一成有界 ToolResult；
6. 输出超限时停止继续积累，并设置 `truncated`；
7. Windows 使用 Job Object 终止进程树；
8. POSIX 使用 process group 终止子进程树；
9. timeout、cancel、spawn failure、non-zero exit 分别返回结构化 error_kind；
10. 不把继承的 API key、代理密码和完整环境变量写入结果；
11. 每个运行进程都由 `ToolProcessHandle` 跟踪，Actor 终态时清理。

#### 测试

- 正常命令和 exit code 0；
- stderr；
- non-zero exit；
- cwd root 内执行；
- cwd 越界拒绝；
- timeout kill；
- CancellationToken kill tree；
- 输出上限；
- 子进程不会在 Actor 结束后遗留；
- Windows 和 POSIX 分支分别测试。

### 步骤 4：实现 approval policy 和 ApprovalManager

#### 参考代码

- `tools/approval.py:detect_dangerous_command`；
- `tools/approval.py:submit_pending/list_gateway_approvals/resolve_gateway_approval`；
- `tools/approval.py:_get_approval_timeout`；
- `tools/approval.py:is_approved`；
- `tui_gateway/server.py` 的 `approval.request` 和 `approval.respond`；
- `sagent-agent/src/command.rs` 的 `ApprovalDecision`；
- `sagent-agent/src/transition.rs` 的 `AwaitingApproval` 转移。

#### 创建/修改文件

```text
crates/sagent-tools/src/approval_policy.rs
crates/sagent-runtime/src/approval.rs
crates/sagent-runtime/src/input.rs
crates/sagent-runtime/src/event.rs
crates/sagent-runtime/src/actor.rs
crates/sagent-runtime/tests/approval_actor.rs
```

#### 实现规则

1. `ApprovalPolicy::classify(tool, args)` 返回 Allow、RequireApproval 或 Deny；
2. read_file 默认只要路径安全即可执行；
3. terminal 危险命令必须 RequireApproval；
4. Deny 不进入等待队列，也不能通过 ResolveApproval 绕过；
5. pending approval 只归属于当前 SessionActor；
6. approval_id、turn_id、tool_call_id、session_id 必须全部匹配；
7. `Once` 只影响当前调用；
8. `Session` 只影响当前 Session 的同类 policy_key；
9. `Always` 必须保存为明确的用户策略，不能由模型参数产生；
10. `Deny`、timeout、interrupt 都必须唤醒等待者；
11. `interactive_approval=false` 时，不得阻塞等待不存在的 UI，返回稳定 capability/approval 错误；
12. approval timeout 使用配置值，设置最小和最大边界；
13. 不使用进程全局 mutable map 保存待审批状态。

#### 事件

新增或扩展：

```text
ApprovalRequested
ApprovalResolved
ApprovalTimedOut
ToolStarted
ToolCompleted
```

事件必须包含 `session_id`，可归属 Turn 的事件必须包含 `turn_id`，审批事件还必须包含 `approval_id` 和 `tool_call_id`。

#### 测试

- Once/Session/Always/Deny；
- 错误 session_id；
- 错误 approval_id；
- 重复 resolve；
- timeout；
- interrupt 唤醒等待；
- 不支持 interactive approval 的客户端；
- 两个 Session 的 approval 不串线；
- approval 事件可以序列化和恢复。

### 步骤 5：接入 Provider tool-call 与 Actor 工具回环

#### 参考代码

- `sagent_provider::ProviderEvent::ToolCallDelta`；
- `model_tools.py:handle_function_call`；
- `run_agent.py:_execute_tool_calls_sequential`；
- `agent/tool_executor.py`；
- `agent/transports/chat_completions.py` 的 assistant tool-call 转换；
- `sagent-agent/src/transcript.rs`。

#### 修改文件

```text
crates/sagent-runtime/src/provider_worker.rs
crates/sagent-runtime/src/input.rs
crates/sagent-runtime/src/actor.rs
crates/sagent-runtime/src/tool_worker.rs
crates/sagent-runtime/tests/tool_actor.rs
```

#### 实现顺序

1. `RuntimeProviderSink` 按 tool-call index 聚合 delta；
2. 完成时校验 id、name 和 arguments JSON；
3. 将完整 `ToolCall` 发送给 Actor，不在 Provider worker 执行工具；
4. Actor 校验 generation 的 tool schema hash；
5. Actor 持久化 assistant tool-call 消息；
6. Actor 通过 registry 查找 handler；
7. 按顺序执行一个或多个工具调用；第一版不做并行执行；
8. 工具需要 approval 时暂停 Turn，不启动 tool worker；
9. 工具完成后通过 `WorkerEvent::ToolResult` 回 Actor；
10. Actor 调用 Store 提交 tool message；
11. 将 tool message 加入下一次 PromptSnapshot；
12. 再次调用 Provider，直到 final text、失败、取消或达到最大 tool rounds；
13. 对重复 ToolCallId、重复 tool result、未知工具、非法参数统一失败收口；
14. 达到最大回环次数时使用明确的 `tool_loop_limit` 失败原因。

#### 关键约束

- assistant(tool_calls) 必须先于对应 tool message；
- tool message 必须带同一个 tool_call_id；
- tool message 不能直接作为最终 assistant；
- provider worker 不持有 Store；
- tool worker 不发布 RuntimeEvent；
- late result 必须按 turn_id、tool_call_id 和 active 状态丢弃。

### 步骤 6：补充 Store 工具消息和事件持久化

#### 参考代码

- `hermes_state.py` 的 tool message 写入和 transcript 恢复；
- `crates/sagent-store/src/turn.rs::commit_tool_result`；
- `crates/sagent-store/src/write.rs::NewMessage`；
- `crates/sagent-store/src/event.rs`；
- `crates/sagent-agent/src/transcript.rs`。

#### 修改文件

```text
crates/sagent-store/src/turn.rs
crates/sagent-store/src/write.rs
crates/sagent-store/src/event.rs
crates/sagent-store/src/migration.rs   # 只有确有需要时增加 migration
crates/sagent-store/tests/tool_result.rs
```

#### 实现内容

1. 增加 assistant tool-call 的原子写入 API；
2. assistant tool-call 写入 `tool_calls`、`tool_name`、`finish_reason=tool_calls`；
3. 增加 tool result 写入的成功/失败元数据；
4. `commit_tool_result` 必须校验 Turn、Session、ToolCallId 和 running 状态；
5. 重复提交返回稳定错误，不能生成第二条 tool message；
6. `tool.completed` 与 tool message 使用同一事务；
7. approval request/resolve/timeout 作为可恢复 daemon event 保存；
8. delta、spinner、进程输出实时片段不写入 daemon_events；
9. 恢复时按 event sequence 和消息顺序重建 pending tool/approval；
10. 已完成的 tool call 恢复后不得再次执行；
11. Store 不保存 API key、完整环境变量和未脱敏命令。

#### 测试

- assistant tool-call + tool result 顺序；
- 成功和失败 tool result；
- 重复 tool_call_id；
- 不属于当前 Turn 的 tool result；
- Turn 非 running 时写入；
- 事务中途失败回滚；
- 进程重启后恢复 pending approval；
- 恢复后已完成工具不会重跑；
- FTS 不把内部 approval payload 当普通用户消息搜索结果。

### 步骤 7：实现取消、超时和终态竞争

#### 实现内容

1. `SessionActor` 为 Provider、Tool、Approval 分别创建 child cancellation token；
2. Turn interrupt 同时取消所有 child token；
3. terminal runner 先发 graceful terminate，再执行平台级 kill tree；
4. approval waiter 收到 cancel 后立即返回，不等待 timeout；
5. Actor 只允许 `Completed`、`Failed`、`Interrupted` 中一个终态成功写入；
6. final 与 tool result、cancel、timeout 的迟到事件全部按 active 状态丢弃；
7. close 时清理所有 pending approval 和 process handles；
8. worker panic/JoinError 统一为 tool/provider failure；
9. 不为取消生成空 assistant 或伪造 tool result。

#### 测试

- Provider 等待时 interrupt；
- terminal 执行时 interrupt；
- approval 等待时 interrupt；
- timeout 与 resolve 同时发生；
- final 与 interrupt 同时发生；
- tool result 与 interrupt 同时发生；
- actor close 后没有孤儿进程和待审批项。

### 步骤 8：RuntimeEvent、协议 DTO 和恢复快照

#### 修改文件

```text
crates/sagent-runtime/src/event.rs
crates/sagent-runtime/src/input.rs
crates/sagent-protocol/src/method/approval.rs
crates/sagent-protocol/src/method/session.rs
crates/sagent-protocol/src/error.rs
```

#### 实现内容

1. 增加稳定的 tool/approval RuntimeEvent；
2. 每个事件包含 session、turn、request/tool/approval 关联字段；
3. 增加 approval request/response DTO，但不在 4.5 接入 RPC dispatch；
4. capability 不足时返回结构化错误，而不是静默等待；
5. 事件序列化使用 snake_case 和稳定字段名；
6. `events_since` 后续可以重放 tool.completed、approval.resolved 等事实事件；
7. delta 和进程输出仍然是瞬态事件，不进入历史重放。

#### 测试

- 事件 JSON round-trip；
- 缺少 capability 的稳定错误；
- session/turn/tool/approval 关联字段完整；
- 重放事件不会重复执行工具；
- 未知事件类型不会破坏恢复。

### 步骤 9：端到端测试与质量门禁

#### 必须新增的测试

1. Mock Provider 发起 `read_file`，结果回到 Provider，最终 assistant 完成；
2. Mock Provider 发起危险 terminal，未 approval 前没有进程启动；
3. Once approval 后只执行一次；
4. Deny 产生可见 tool error 或确定 TurnFailed；
5. Session approval 只影响同一 Session；
6. 两个 Session 同时执行 terminal，事件不串线；
7. read_file 越界、binary、超限和取消；
8. terminal timeout、非零退出、stderr 和输出截断；
9. ToolCallId 重复、未知工具、非法 JSON 参数；
10. tool result 持久化后重新打开数据库恢复；
11. approval timeout、interrupt 和 final race；
12. Provider → tool → Provider → final 的完整回环；
13. API key、环境变量和敏感命令不会出现在事件/日志；
14. DeepSeek/OpenAI smoke test 继续只作为显式 ignored 测试。

#### 质量门禁

```text
cargo fmt --all -- --check
cargo test -p sagent-tools --offline
cargo test -p sagent-store --offline
cargo test -p sagent-runtime --offline
cargo test --workspace --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
git diff --check
```

## 9. 推荐提交顺序

每次提交只改变一个行为边界：

```text
feat(tools): add tool definitions and canonical registry
feat(tools): implement safe read_file
feat(tools): add terminal process supervision
feat(runtime): add approval lifecycle and capability checks
feat(runtime): bridge provider tool calls to tool workers
feat(store): persist assistant tool calls and tool results
test(runtime): cover tool loop cancellation and recovery
test(e2e): cover approval and tool transcript order
```

不要把 registry、terminal、approval、Store migration 和 RPC dispatch 放在一个提交中。

## 10. 风险与明确取舍

### 工具 schema 与 prompt cache

工具 schema 是 generation 的一部分。工具集合、schema 或权限发生变化时，必须产生新的 tool schema hash，不能继续复用旧 PromptSnapshot。

### terminal 安全

第一版宁可拒绝不确定的命令，也不能因为检测器不完整而自动放行危险命令。不要通过 `--yolo`、进程环境变量或模型参数绕过 approval。

### approval 生命周期

approval 是 Session/Turn 领域状态，不是一个全局异步 channel。Actor 退出、Session interrupt 或客户端断开后，所有 pending approval 必须可确定地结束。

### 工具输出

工具输出是模型输入的一部分，必须有大小上限、敏感信息过滤和错误类型。输出截断不能伪装成成功的完整结果。

### 并发工具调用

Python 支持更复杂的批量/并行执行，但 4.5 第一版采用顺序执行，先保证 transcript 顺序、取消和 approval 语义稳定；并行工具作为后续独立设计。

## 11. 4.5 完成验收

满足以下条件才算 4.5 完成：

- [ ] `sagent-tools` registry、schema hash 和错误类型稳定；
- [ ] `read_file` 通过 root、symlink、binary、size 和 cancel 测试；
- [ ] terminal 具备 timeout、output limit、process-tree cancel；
- [ ] 危险 terminal 未审批前不会执行；
- [ ] ApprovalDecision 四种策略行为明确；
- [ ] approval timeout、deny、interrupt 和重复 resolve 有确定结果；
- [ ] Provider tool-call delta 可以聚合成完整 ToolCall；
- [ ] tool result 与 tool_call_id 严格关联；
- [ ] assistant tool-call 和 tool message 顺序正确并可恢复；
- [ ] tool loop 有最大轮数和取消边界；
- [ ] Provider/Tool/Approval worker 不直接写 Store；
- [ ] 两个 Session 并发时工具和审批事件不串线；
- [ ] API key、环境变量和敏感命令不进入日志/事件；
- [ ] workspace 测试、Clippy 和 diff check 全部通过；
- [ ] 4.6 可以在不修改工具核心逻辑的前提下接入公开 RPC。

## 12. 预计交付文件

```text
crates/sagent-tools/
├── Cargo.toml
├── src/lib.rs
├── src/definition.rs
├── src/registry.rs
├── src/schema.rs
├── src/error.rs
├── src/workspace.rs
├── src/read_file.rs
├── src/process.rs
├── src/terminal.rs
├── src/command_policy.rs
├── src/approval_policy.rs
└── tests/

crates/sagent-runtime/
├── src/approval.rs
├── src/tool_worker.rs
├── src/actor.rs              # 工具回环和 ResolveApproval
├── src/input.rs              # ToolCall/ToolResult/Approval worker event
├── src/event.rs              # tool/approval RuntimeEvent
└── tests/tool_actor.rs

crates/sagent-store/
├── src/turn.rs               # assistant tool-call/tool result 事务
├── src/event.rs              # tool/approval daemon event
└── tests/tool_result.rs

crates/sagent-protocol/
└── src/method/approval.rs    # 4.5 DTO；4.6 接 dispatch
```

4.5 的实现顺序必须保持：先工具契约，再单工具，再 terminal，再 approval，再 Actor 回环，最后做 Store 恢复和端到端测试。不能先改 TUI 或 RPC 来反向决定工具行为。
