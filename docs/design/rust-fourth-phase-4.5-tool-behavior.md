# Sagent 4.5 工具行为契约与测试 Fixture

作者：SongZQ  
状态：步骤 0 已完成  
依据：`rust-fourth-phase-4.5-plan.md`

本文冻结 4.5 第一版工具、terminal、approval 和 tool-call 回环的行为边界。它是 Rust 实现的契约，不是对 Python 全部工具的复制。

## 1. Python 到 Rust 的参考映射

| Python 代码 | 需要保持的行为 | Rust 落点 |
| --- | --- | --- |
| `model_tools.py:handle_function_call` | 工具名、参数、未知工具和异常结果 | `sagent-tools::ToolRegistry`、Runtime dispatcher |
| `toolsets.py` | 工具集合和 schema 暴露边界 | `ToolSchemaSet`、generation hash |
| `tools/registry.py` | 注册、重复名称、schema、可用性筛选 | `ToolRegistry` |
| `tools/file_tools.py:read_file_tool` | path、offset、limit、截断、错误 | `sagent-tools::read_file` |
| `tools/file_tools.py:_truncate_to_char_budget` | 输出字符上限和 truncated 标记 | `ToolResult::truncated` |
| `tools/path_security.py:validate_within_dir` | workspace root containment | `WorkspaceRoot` |
| `tools/terminal_tool.py:terminal_tool` | command、cwd、timeout、输出和退出码 | `sagent-tools::terminal` |
| `tools/terminal_tool.py:_run_approval_guards` | 执行前危险命令检查 | `ApprovalPolicy` |
| `tools/terminal_tool.py:_run_foreground` | 前台进程输出和超时 | `ProcessRunner` |
| `tools/approval.py:check_dangerous_command` | 命令风险分类 | `ApprovalPolicy::classify` |
| `tools/approval.py:request_tool_approval` | 工具 approval 请求 | `ApprovalManager` |
| `tools/approval.py:resolve_gateway_approval` | approval 决定匹配和唤醒 | `ResolveApproval` |
| `run_agent.py:_execute_tool_calls_sequential` | tool-call → result → 下一次模型调用 | `SessionActor` tool loop |
| `hermes_state.py` | tool message 与事务边界 | `sagent-store` 工具消息事务 |

## 2. 统一结果约定

### 2.1 ToolCall

```json
{
  "id": "tool-call-id",
  "name": "read_file",
  "arguments": {"path": "src/main.rs"}
}
```

约束：

- `id` 在一个 Turn 内唯一；
- `name` 必须已注册；
- `arguments` 必须是 JSON object；
- Provider 的多个 delta 必须按顺序合并；
- JSON 解析失败时不能启动工具；
- 未知工具、重复 id 和非法参数都必须返回结构化错误。

### 2.2 ToolResult

```json
{
  "tool_call_id": "tool-call-id",
  "name": "read_file",
  "ok": true,
  "content": "file content",
  "truncated": false,
  "exit_code": null,
  "error_kind": null
}
```

失败示例：

```json
{
  "tool_call_id": "tool-call-id",
  "name": "terminal",
  "ok": false,
  "content": "命令执行超时",
  "truncated": false,
  "exit_code": null,
  "error_kind": "timeout"
}
```

`content` 必须有大小上限，不能包含 API key、完整环境变量或未经脱敏的秘密。

### 2.3 ApprovalRequest

```json
{
  "approval_id": "approval-id",
  "session_id": "session-1",
  "turn_id": "turn-1",
  "tool_call_id": "tool-call-id",
  "tool_name": "terminal",
  "summary": "执行受保护的 terminal 命令",
  "policy_key": "terminal:differential-risk",
  "expires_at": "2026-09-05T12:00:00Z"
}
```

展示给客户端的 `summary` 必须脱敏。审批状态必须绑定 `session_id`、`turn_id`、`approval_id` 和 `tool_call_id`。

## 3. read_file 行为契约

### 3.1 输入

```json
{
  "path": "src/main.rs",
  "offset": 1,
  "limit": 2000
}
```

第一版保留 Python 的 `offset`/`limit` 语义，但 Rust 工具仍然必须对总字节数和总字符数设上限。该分页只用于普通文件读取，不用于要求模型完整读取的 instruction/skill 文件。

### 3.2 行为矩阵

| 场景 | `ok` | `error_kind` | 是否写数据库 |
| --- | --- | --- | --- |
| root 内 UTF-8 普通文件 | `true` | `null` | 否 |
| 文件不存在 | `false` | `not_found` | 否 |
| 路径包含 `..` 越界 | `false` | `path_denied` | 否 |
| canonical path 逃出 root | `false` | `path_denied` | 否 |
| 符号链接逃出 root | `false` | `path_denied` | 否 |
| 目标是目录 | `false` | `not_file` | 否 |
| 二进制文件 | `false` | `binary_file` | 否 |
| 文件超过大小限制 | `false` | `file_too_large` | 否 |
| 输出超过字符限制 | `true` | `null` | 否 |
| CancellationToken 已取消 | `false` | `cancelled` | 否 |

输出超出字符限制时，允许返回 `ok=true`、`truncated=true`，但必须在 `content` 末尾增加明确的截断说明。

### 3.3 安全规则

1. 相对路径相对于显式 `WorkspaceRoot` 解析；
2. 不接受未经过 root 校验的绝对路径；
3. 不通过 shell 读取文件；
4. 不将原始二进制 bytes 直接传给模型；
5. 不把文件路径之外的宿主机信息写入结果；
6. 文件读取取消后不留下后台任务。

## 4. terminal 行为契约

### 4.1 输入

```json
{
  "command": "cargo test -p sagent-tools",
  "cwd": ".",
  "timeout_ms": 30000,
  "output_limit": 32768
}
```

### 4.2 行为矩阵

| 场景 | 进程是否启动 | `error_kind` | Turn 结果 |
| --- | --- | --- | --- |
| 安全命令 | 是 | `null` | 继续 |
| 危险命令且未审批 | 否 | `approval_required` | AwaitingApproval |
| approval Deny | 否 | `approval_denied` | 工具失败或 TurnFailed |
| cwd 越界 | 否 | `path_denied` | 工具失败 |
| spawn 失败 | 否/未知 | `spawn_failed` | 工具失败 |
| exit code 0 | 是 | `null` | 继续 |
| 非零 exit code | 是 | `non_zero_exit` | 工具失败 |
| timeout | 被终止 | `timeout` | 工具失败 |
| interrupt | 被终止 | `cancelled` | TurnInterrupted |
| 输出超过限制 | 是 | `null` | 继续，但 `truncated=true` |

### 4.3 进程生命周期

```text
ToolCall
  → approval policy
  → spawn
  → register process handle
  → read stdout/stderr with limits
  → exit/timeout/cancel
  → unregister process handle
  → emit ToolResult
```

Windows 必须终止 Job Object 中的进程树；POSIX 必须终止 process group。Actor 结束时，不能遗留工具子进程。

## 5. Approval 行为契约

### 5.1 决策含义

| 决策 | 有效范围 | 是否持久保存策略 |
| --- | --- | --- |
| `once` | 当前 ToolCall | 否 |
| `session` | 当前 Session 的同类 policy key | 是，Session 状态 |
| `always` | 明确的长期用户策略 | 是，配置/策略存储 |
| `deny` | 当前 ToolCall | 否 |

### 5.2 状态转移

```text
RunningTool
  → ApprovalRequested
  → AwaitingApproval
      ├─ once/session/always → ApprovalResolved → RunningTool
      ├─ deny                 → ApprovalResolved → Failed
      ├─ timeout              → ApprovalTimedOut → Failed
      └─ interrupt            → Interrupted
```

### 5.3 必须拒绝的情况

- approval_id 不属于当前 Session；
- approval_id 不属于当前 Turn；
- tool_call_id 不匹配；
- 当前已经不是 `AwaitingApproval`；
- 客户端没有 `interactive_approval` 能力；
- approval 已经被 resolve 或 timeout；
- 工具策略明确为 Deny。

Approval 等待不能依赖进程全局变量，也不能因为客户端断开而永久阻塞 Actor。

## 6. Tool-call 回环契约

正常回环顺序：

```text
user
assistant(tool_calls)
tool
assistant(final)
```

Runtime 时序：

```text
Provider ToolCallDelta
  → Provider worker 聚合
  → WorkerEvent::ToolCallReady
  → Actor 校验 schema/id/arguments
  → Store 写 assistant(tool_calls)
  → ToolStarted
  → ToolResult
  → Store 写 tool message
  → 新 PromptSnapshot
  → Provider 再次 stream
```

不变量：

1. assistant tool-call 必须先于 tool message；
2. tool message 必须带匹配的 `tool_call_id`；
3. 一个 ToolCallId 只能成功提交一次结果；
4. 未知工具不能执行；
5. tool schema hash 变化不能复用旧 PromptSnapshot；
6. tool loop 必须有最大轮数；
7. Provider、Tool worker 不直接写 Store；
8. late result 必须按 `turn_id`、`tool_call_id` 和 active 状态丢弃。

## 7. 持久化契约

### 7.1 messages

assistant tool-call 消息：

```text
role = assistant
tool_calls = JSON array
finish_reason = tool_calls
```

tool result 消息：

```text
role = tool
tool_call_id = 原始 ToolCallId
tool_name = 工具名
content = 有上限的结果文本
```

### 7.2 daemon_events

允许持久化：

```text
tool.requested
tool.started
tool.completed
approval.requested
approval.resolved
approval.timed_out
turn.interrupted
turn.failed
```

不持久化：

```text
ModelTextDelta
终端逐字节输出
spinner/typing
UI 局部状态
```

### 7.3 恢复规则

- 已完成的 tool result 恢复后不能再次执行；
- pending approval 恢复后必须重新发布 approval.request；
- 已取消或失败的 Turn 不能继续执行工具；
- 重复 event replay 不能生成重复 tool message；
- 恢复后的 PromptSnapshot 必须重新验证 tool schema hash。

## 8. 步骤 0 验收结果

- [x] Python 参考文件和函数已经列出；
- [x] read_file 行为矩阵已经定义；
- [x] terminal 行为矩阵已经定义；
- [x] approval 决策、超时和取消规则已经定义；
- [x] ToolCall/ToolResult 关联规则已经定义；
- [x] assistant(tool_calls) → tool → assistant(final) 顺序已经定义；
- [x] Store 和 daemon_events 持久化边界已经定义；
- [x] 取消、late event 和重复结果规则已经定义；
- [x] 已创建下一阶段需要的 fixture 目录和 JSON 文件；
- [x] 未修改 Provider、Runtime 或 Store 生产逻辑。

步骤 0 完成后进入步骤 1：创建 `sagent-tools` crate，实现 `ToolDefinition`、`ToolRegistry`、canonical schema 和 `tool_schema_hash`。
