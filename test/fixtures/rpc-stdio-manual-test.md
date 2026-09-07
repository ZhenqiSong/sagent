# sagent-rpc 手工 NDJSON 协议测试

本文件用于单独启动 `sagent-rpc` 后，在其标准输入中**逐行**输入 JSON-RPC 请求。
它模拟 TUI 的协议行为，覆盖 transport、握手、会话、普通流式 Turn、事件回放、
中断、工具审批与错误路径。

## 前置条件

在 `D:\projects\sagent` 中用 VS Code 的“调试 RPC 服务端（D:\sagent-smoke）”启动，
或运行：

```powershell
cargo run --package=sagent-rpc --bin=sagent-rpc -- --home D:\sagent-smoke
```

进程首先会输出一条 `gateway.ready` event。这是服务端通知，不需要也不能回复。

每个 JSON 对象都必须独占一行。下面的 `替换为……` 是占位符，输入前必须替换成响应中
实际返回的值；不要把 Markdown 注释一起粘贴到终端。

本测试会创建名为 `rpc-manual-debug` 的会话，并可能调用已配置的模型。完成后可用 CLI
归档该会话，避免干扰日常会话列表。

## A. Transport 与解析错误

在 `stdio.rs` 的 `reader_loop` 和 `dispatcher_loop` 下断点。

### A1. 服务端公开 ping

```json
{"jsonrpc":"2.0","id":1,"method":"gateway.ping","params":{}}
```

预期：返回相同 `id: 1`，且 `result.ok` 为 `true`。该方法不要求 `client.hello`。

### A2. 未知方法

```json
{"jsonrpc":"2.0","id":2,"method":"debug.unknown","params":{}}
```

预期：返回 JSON-RPC `method not found` 错误；进程不退出，下一条有效请求仍可处理。

### A3. 参数类型错误

```json
{"jsonrpc":"2.0","id":3,"method":"gateway.ping","params":[]}
```

预期：返回 `invalid params` 错误。数组不能绕过 DTO 的对象参数校验。

### A4. 非法 JSON

```text
{"jsonrpc":"2.0","id":4,"method":
```

预期：返回 parse error；随后继续输入 A1 的 ping，仍应成功。这验证一条坏帧不会污染
后续 NDJSON 请求。

## B. 握手与访问控制

### B1. 未握手的交互请求

```json
{"jsonrpc":"2.0","id":10,"method":"session.create","params":{"title":"should-fail-before-hello"}}
```

预期：错误码 `-32006`，即 handshake required。该失败不能让连接进入已握手状态。

### B2. 完成 client.hello

```json
{"jsonrpc":"2.0","id":11,"method":"client.hello","params":{"protocol_version":1,"client_id":"00000000-0000-4000-8000-000000000001","surface":"tui","capabilities":{"interactive_approval":true,"supports_stream_edits":false}}}
```

预期：`result.protocol_version` 为 `1`，并返回 features、capabilities、session_policy。
在 `connection.rs` 的 `ConnectionState::dispatch` 下断点，可确认 capability 只写入本连接。

### B3. 再次读取会话列表

```json
{"jsonrpc":"2.0","id":12,"method":"session.list","params":{"include_archived":false,"limit":20,"offset":0}}
```

预期：返回当前 Profile 的会话摘要；该结果只读取数据库，不启动 SessionActor。

## C. 会话创建、恢复与普通流式 Turn

### C1. 创建隔离会话

```json
{"jsonrpc":"2.0","id":20,"method":"session.create","params":{"title":"rpc-manual-debug"}}
```

从响应复制 `result.session_id`，下文统一称为 `SESSION_ID`。

### C2. 恢复空会话

```json
{"jsonrpc":"2.0","id":21,"method":"session.resume","params":{"session_id":"替换为 SESSION_ID","message_limit":20,"message_offset":0}}
```

预期：`messages` 是空数组。这一步验证 TUI 进入会话时读取的是权威 snapshot。

### C3. 提交最小模型问题

```json
{"jsonrpc":"2.0","id":22,"method":"prompt.submit","params":{"session_id":"替换为 SESSION_ID","text":"请只回复 OK，不要调用工具。"}}
```

预期先得到 `id: 22` 的响应，其中包含 `status: "streaming"` 与 `turn_id`。随后同一 stdout
会出现 event，常见顺序为：

```text
prompt.accepted
message.user.persisted
message.delta
model.usage
message.complete
turn.completed
```

在 `stdio.rs` 的 `dispatch_prompt_request`、`event_bridge.rs` 的 `subscription.recv()` 附近
下断点，分别观察请求进入 Actor 与 RuntimeEvent 被转换为 JSON-RPC event。

从响应复制 `turn_id`，下文称为 `TURN_ID`；等待 `turn.completed` 后再执行下一节。

### C4. 读取最终快照

```json
{"jsonrpc":"2.0","id":23,"method":"session.resume","params":{"session_id":"替换为 SESSION_ID","message_limit":20,"message_offset":0}}
```

预期：`messages` 中存在 user 与 assistant 两条持久化消息。不要把之前收到的
`message.delta` 当作最终消息；最终文本应以这个 snapshot 为准。

## D. 持久化事件回放与幂等消费

### D1. 从起点读取事件

```json
{"jsonrpc":"2.0","id":30,"method":"session.events.since","params":{"session_id":"替换为 SESSION_ID","after_sequence":0,"limit":100}}
```

预期：返回此会话的持久化事实和 `latest_sequence`。记录最后一条 event 的 `sequence`，称为
`LAST_SEQUENCE`。高频 `message.delta` 不应位于这个列表。

### D2. 使用 checkpoint 补读

```json
{"jsonrpc":"2.0","id":31,"method":"session.events.since","params":{"session_id":"替换为 SESSION_ID","after_sequence":替换为 LAST_SEQUENCE,"limit":100}}
```

预期：没有新事件时返回空 events。此请求模拟 TUI 重连后的 checkpoint 补读。

### D3. session_id 不存在

```json
{"jsonrpc":"2.0","id":32,"method":"session.events.since","params":{"session_id":"missing-session","after_sequence":0,"limit":10}}
```

预期：稳定的 session not found 错误，且连接可继续使用。

## E. 同会话忙碌与中断

本节会再次调用模型。为了能在模型完成前中断，请先发送长输出请求，收到 `streaming` 响应后
立即输入中断请求。若模型过快完成而中断返回“没有活跃 Turn”，重新创建会话再试即可。

### E1. 启动长输出 Turn

```json
{"jsonrpc":"2.0","id":40,"method":"prompt.submit","params":{"session_id":"替换为 SESSION_ID","text":"请连续输出一篇较长的 Rust 异步编程教程，至少 2000 个汉字，不要调用工具。"}}
```

可在仍生成时额外发送一条 prompt，验证同一 Session 的 busy policy：

```json
{"jsonrpc":"2.0","id":41,"method":"prompt.submit","params":{"session_id":"替换为 SESSION_ID","text":"这条请求应因会话忙碌而被拒绝。"}}
```

预期：第二条得到 session busy 错误，不会隐式排队。

### E2. 请求中断

```json
{"jsonrpc":"2.0","id":42,"method":"session.interrupt","params":{"session_id":"替换为 SESSION_ID"}}
```

预期：即时响应只代表 interrupt 已被 runtime 接受；真正结束仍要等待
`turn.interrupted` 或其它 terminal event。随后再次调用 `session.resume`，验证最终数据库快照。

## F. 审批与工具事件（有前置条件）

审批不是靠伪造 `approval.respond` 就能测试：必须先有已配置的工具、该工具的审批策略，且模型
确实选择调用该工具。当前测试环境若没有这类工具，跳过本节是正确结果。

当你实际收到 `approval.requested` event 时，从 payload 复制 `approval_id` 与 `turn_id`，再输入：

```json
{"jsonrpc":"2.0","id":50,"method":"approval.respond","params":{"session_id":"替换为 SESSION_ID","turn_id":"替换为 approval.requested 的 turn_id","approval_id":"替换为 approval_id","decision":"deny"}}
```

预期：即时响应 `status: "accepted"`；随后收到 `approval.resolved`，并由工具/Turn 的后续
event 宣布最终结果。这里选择 `deny`，避免手工调试意外执行外部副作用。

若需要验证允许路径，将 `decision` 改为 `once`；不要在手工测试中使用 `always`，它会改变
持久化策略。

## G. 连接结束

结束手工测试时在终端按 Ctrl+C，或关闭 stdin。RPC 进程应有序结束；stdout 不应混入日志。
下次启动是一个全新的 ConnectionState，必须重新执行 B2 的 `client.hello`。
