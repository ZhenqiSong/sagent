# Sagent Actor 全流程详解

作者：SongZQ

本文说明 Rust 重写项目 4.3 中的 SessionSupervisor、SessionHandle、SessionActor、ActiveTurn、Worker、Store 和 RuntimeEvent 如何协作。示例使用 session_id = chat-001。

## 1. Actor 的职责

一个 Session 对应一个 Actor。Actor 像该会话专属的小管家：拥有可变 Turn 状态、独占 Store，并按 mailbox 顺序处理命令。

    CLI / RPC / TUI
           │ SessionHandle
           ▼
    SessionSupervisor
           │ SessionId -> ManagedSession
           ▼
    SessionActor mailbox
           ├── SessionCommand
           ├── WorkerEvent
           └── WorkerExited
                  │
                  ├── sagent-agent 状态规则
                  └── sagent-store 原子事务

同一个 Session 内部是串行的；不同 Session 的 Actor 可以并行运行。因此不需要全局 Mutex<Store>，也不会让会话 A 的慢 Worker 阻塞会话 B。

## 2. 主要对象

### 2.1 SessionSupervisor

Supervisor 维护：

    SessionId -> ManagedSession

ManagedSession 保存命令 mailbox 的 Sender、事件广播发送端和 Actor 的 JoinHandle。

调用：

    let handle = supervisor
        .get_or_start(SessionId::new("chat-001"))
        .await?;

首次调用会：

1. 打开该 Session 独占的 Store；
2. 创建容量为 32 的有界 mailbox；
3. 创建 RuntimeEvent 广播通道；
4. 创建并启动 SessionActor；
5. 将 Actor 放入映射；
6. 返回 SessionHandle。

同一个 Session 并发调用 100 次 get_or_start，只能启动一个 Actor，其余调用复用同一个 mailbox。

### 2.2 SessionHandle

Handle 是 CLI、RPC 和 TUI 的唯一入口：

    submit(request_id, input).await
    interrupt(request_id).await
    close().await
    subscribe()

Handle 不暴露 SQLite 连接、Store、ActiveTurn 或 Worker，因此外部不能绕过 Actor 写数据库。

### 2.3 ActiveTurn

Actor 的 active 保存当前回合：

    turn_id
    request_id
    generation
    TurnState
    CancellationToken
    worker monitor JoinHandle
    worker AbortHandle
    terminal

terminal 用于保证 Final、Failed、Cancelled 和 Interrupt 竞争时只接受一个终态。

## 3. 启动会话

假设数据库中已有：

    session_id = chat-001

执行 get_or_start 后只准备运行时资源，不会自动创建 Turn：

    Supervisor map：存在 chat-001
    Actor active：None
    turns：没有 running Turn

如果 Store 打开失败，统一返回 RuntimeError::Persistence，不把 rusqlite 类型泄漏给 RPC 或 TUI。

## 4. SubmitPrompt 完整流程

用户发送：

    你好，请介绍 Rust ownership

调用：

    let receipt = handle
        .submit(
            RequestId::new(),
            UserInput::new("你好，请介绍 Rust ownership")?,
        )
        .await?;

### 4.1 投递 mailbox

Handle 使用有界 try_send：

    投递成功       -> 等待一次性 accepted 回执
    mailbox 已满   -> MailboxFull
    mailbox 已关闭 -> ActorStopped

### 4.2 busy 检查

当 active = None 时继续；当已有 active Turn 时立即返回：

    RuntimeError::Busy { session_id }

busy 请求绝对不会产生 user message、Turn 或 daemon event。

### 4.3 PromptSnapshot 和 generation

Actor 组装 SystemPromptParts、system message 和 user message，创建 PromptSnapshot，得到稳定的 system_prompt_hash。

普通会话只使用 generation=0：

    第一次提交：不存在 generation=0 -> create_generation
    后续提交：复用 generation=0
    hash 不一致：RequiresTransition

不能在会话中途静默替换系统提示词或工具 schema；压缩等场景必须显式创建新的 generation。

### 4.4 begin_turn 原子事务

Actor 调用：

    store.begin_turn(&start_turn, &user_message)?;

一个事务中完成：

    INSERT INTO messages (... role = 'user' ...);
    UPDATE sessions SET message_count = message_count + 1;
    INSERT INTO turns (... status = 'running' ...);
    INSERT INTO daemon_events (... event_type = 'turn.started' ...);
    INSERT INTO daemon_events (... event_type = 'message.committed' ...);
    COMMIT;

只有 COMMIT 成功之后才会：

1. 创建 CancellationToken；
2. 启动受监管 Worker；
3. 设置 active = Some(ActiveTurn)；
4. 发布 PromptAccepted；
5. 发布 UserMessagePersisted；
6. 返回 SubmitReceipt。

所以客户端收到 accepted 时，user 消息和 running Turn 已经存在。

## 5. Worker 生命周期

Worker 只持有 Actor Sender、TurnId 和 CancellationToken 子 token，不持有 Store，也不执行 SQL。

Worker 可以发送：

    WorkerEvent::TextDelta { turn_id, text }
    WorkerEvent::FinalText { turn_id, text }
    WorkerEvent::Failed { turn_id, reason }
    WorkerEvent::Cancelled { turn_id }

Actor 启动监控任务等待 Worker JoinHandle。正常退出或 panic 都转换为 WorkerExited，再回到 Actor 的串行处理路径。

## 6. 流式 Delta

Worker 发送：

    WorkerEvent::TextDelta {
        turn_id,
        text: "Rust 的所有权".into(),
    }

Actor 只发布实时事件：

    {
      "type": "model_text_delta",
      "session_id": "chat-001",
      "turn_id": "...",
      "data": {"text": "Rust 的所有权"}
    }

Delta 是瞬态事件，不写 daemon_events，避免每个字符创建 SQLite 事务。迟到的 Delta 如果 turn_id 已不再 active，会被忽略。

## 7. FinalText 完成

Worker 发送：

    WorkerEvent::FinalText {
        turn_id,
        text: "所有权是 Rust 的编译期内存管理机制。".into(),
    }

Actor 的顺序是：

    检查 turn_id 和 terminal
        ↓
    创建 assistant NewMessage
        ↓
    Store::complete_turn
        ↓
    停止并回收 Worker
        ↓
    FinalMessagePersisted
        ↓
    TurnCompleted
        ↓
    清理 active

complete_turn 的事务等价于：

    INSERT INTO messages (... role = 'assistant' ...);
    UPDATE sessions SET message_count = message_count + 1;
    UPDATE turns SET status = 'completed', assistant_message_id = ...;
    INSERT INTO daemon_events (... event_type = 'message.committed' ...);
    INSERT INTO daemon_events (... event_type = 'turn.completed' ...);
    COMMIT;

完成事件一定是“先提交数据库，后广播”。收到 TurnCompleted 后，客户端可以放心重新打开 Store 读取 assistant 消息。

## 8. Failed 流程

例如 Provider 不可用：

    WorkerEvent::Failed {
        turn_id,
        reason: "provider unavailable".into(),
    }

Actor 调用：

    store.fail_turn(
        &turn_id,
        "worker",
        "provider unavailable",
        &timestamp,
    )?;

结果：

    turns.status：running -> failed
    messages：只保留 user 消息
    assistant 消息：不存在
    daemon_events：新增 turn.failed

数据库提交成功后才广播 TurnFailed。Worker 正常退出但没有发送 FinalText，也会被视为 worker_exit 失败；Worker panic 则转换为 WorkerFailure。

## 9. Interrupt 流程

用户点击停止：

    handle.interrupt(RequestId::new()).await?;

Actor 按以下顺序处理：

    Interrupt 进入 mailbox
        ↓
    检查 active；没有 active -> NoActiveTurn
        ↓
    cancellation.cancel()
        ↓
    abort 实际 Worker，等待 monitor JoinHandle
        ↓
    Store::interrupt_turn
        ↓
    发布 TurnInterrupted
        ↓
    active = None

数据库操作等价于：

    UPDATE turns
       SET status = 'interrupted',
           completed_at = ...,
           outcome_json = ...
     WHERE turn_id = ... AND status = 'running';

    INSERT INTO daemon_events (... event_type = 'turn.interrupted' ...);
    COMMIT;

Interrupt 不创建 assistant 空消息。重复 Interrupt 不会重复写库。

## 10. Close 流程

### 10.1 空闲关闭

    active = None
    Close -> Closed -> Actor 退出

### 10.2 active Turn 关闭

    Close
      -> 取消 Worker
      -> interrupt_turn
      -> 发布 TurnInterrupted
      -> 返回 Closed
      -> Actor 退出

Supervisor 的 remove 会发送 Close、等待 Actor 结束，再删除 map 条目。旧 Handle 之后返回 ActorStopped。

## 11. Final 与 Interrupt 竞态

所有命令和 WorkerEvent 都由同一个 Actor 串行处理。

### Final 先到

    mailbox: FinalText, Interrupt
    结果：completed；assistant 存在；Interrupt -> NoActiveTurn

### Interrupt 先到

    mailbox: Interrupt, FinalText
    结果：interrupted；没有 assistant；迟到 FinalText 被忽略

原因是所有输入经过同一 mailbox，数据库终态写入和 active 清理都在 Actor 内完成。

## 12. 多会话并行

有两个会话：

    session-a
    session-b

同时提交：

    let (a, b) = tokio::join!(
        handle_a.submit(RequestId::new(), UserInput::new("任务 A")?),
        handle_b.submit(RequestId::new(), UserInput::new("任务 B")?),
    );

结果是 Actor A 只写 session-a，Actor B 只写 session-b。A 的 busy、阻塞 Worker 或 interrupt 不会修改 B 的 active 状态。

## 13. 事件订阅与断线恢复

订阅实时事件：

    let mut subscription = handle.subscribe();
    let event = subscription.recv().await?;

订阅者处理太慢时，broadcast 缓冲区会产生：

    RuntimeEventKind::SubscriberLagged { skipped }

这只表示部分瞬态 Delta 丢失，不表示数据库事实丢失。客户端应保存最后一个持久化 sequence，然后调用：

    store.events_since(&EventQuery {
        session_id,
        after_sequence: last_sequence,
        limit: 200,
    })?;

恢复对象是：

    turn.started
    message.committed
    turn.completed
    turn.interrupted
    turn.failed

不需要恢复每一个 ModelTextDelta。

## 14. 数据库状态示例

一次成功对话：

    sessions:
      message_count = 2

    messages:
      1 | user      | 你好
      2 | assistant | 你好，我可以帮助你。

    turns:
      turn-001 | generation=0 | status=completed

    daemon_events:
      1 | turn.started
      2 | message.committed
      3 | message.committed
      4 | turn.completed

一次中断对话：

    messages:
      1 | user | 执行一个耗时任务

    turns:
      turn-002 | status=interrupted

    daemon_events:
      turn.started
      message.committed
      turn.interrupted

中断不会伪造 assistant 消息。

## 15. Python 对照

    tui_gateway/server.py:_start_inflight_turn
        -> ActiveTurn

    tui_gateway/server.py:_interrupt_busy_session
        -> CancellationToken、Worker abort 和 Interrupt

    tui_gateway/server.py:_run_prompt_submit
        -> SessionActor::submit_prompt

    run_agent.py:AIAgent.interrupt
        -> Actor 的取消路径

    hermes_state.py 消息事务
        -> begin_turn / complete_turn / interrupt_turn / fail_turn

    tui_gateway/server.py:_emit
        -> RuntimeEventSubscription

Python 使用共享 Session 字典和线程；Rust 使用“一个 Session 一个 Actor、一个 Actor 一个 Store、一个 Turn 一个取消令牌”来保证边界。

## 16. 4.3 完成边界

4.3 已完成：

- Supervisor、Handle、Actor 和有界 mailbox；
- generation=0、begin_turn 和 busy 语义；
- Final、Failed、Cancelled、Interrupt 和 Close 收口；
- Worker 取消、panic 监管和终态竞争；
- RuntimeEvent 订阅、lag 诊断和 events_since 恢复测试；
- runtime 单元测试、集成测试、fmt、workspace test 和 clippy 门禁。

4.3 不包含真实 Provider、HTTP/SSE、Tools、Approval、RPC、ratatui、compression、fork 和 retry 的 generation 迁移。这些功能必须通过 Actor 的命令或 WorkerEvent 接入，不能直接写 Store。

## 17. 会话、消息、事件的统一流转模型

理解 Actor 最简单的方法，是把一次用户输入看成一条单向流水线：

```text
客户端
  │  SubmitPrompt(session_id, text)
  ▼
SessionHandle ──发送命令──> SessionActor(mailbox)
                              │
                              ├─读取会话状态（Store）
                              ├─写入 user 消息和 running turn
                              ├─启动 Worker
                              └─广播事件（RuntimeEvent）
                                      │
                                      ▼
                               TUI / RPC / Gateway
```

这里有三个不同的对象，不能混为一谈：

| 对象 | 作用 | 是否持久化 |
| --- | --- | --- |
| 会话（Session） | 对话容器，保存 `session_id`、标题、profile 等元数据 | 是 |
| 消息（Message） | 对话内容，如 user、assistant、system | 是（最终内容） |
| 事件（RuntimeEvent） | 实时通知，如 delta、完成、失败、取消 | 默认否；可用序列号短暂回放 |

事件是“发生了什么”的通知，消息是“最终留下什么”的记录。例如模型输出 100 个 token 时可以产生 100 个 `Delta` 事件，但数据库通常只保存一条完整的 assistant 消息。

## 18. 完整示例：从创建会话到回复完成

假设用户发送：`帮我总结今天的日志`，已有会话 `sess-A`。

### 18.1 初始数据库状态

```text
sessions
┌─────────┬──────────┬─────────────┐
│ id      │ profile  │ active_turn │
├─────────┼──────────┼─────────────┤
│ sess-A  │ default  │ NULL        │
└─────────┴──────────┴─────────────┘

messages
（无本轮消息）
```

客户端调用 `SessionHandle::submit_prompt("帮我总结今天的日志")`。Handle 不直接写数据库，只把 `SubmitPrompt` 放入 Actor 的 mailbox，立即返回一个异步结果。

### 18.2 Actor 接收命令并开启 Turn

Actor 串行处理 mailbox 中的命令：

1. 检查 `active_turn`。为 `NULL`，因此不会返回 `Busy`。
2. 读取当前 generation，例如 `0`。
3. 调用 `Store::begin_turn(session_id, prompt, generation)`，在一个事务中完成：
   - 创建 `turn_id = turn-101`；
   - 插入 user 消息 `message_id = 501`；
   - 创建 running turn 记录；
   - 把 session 的 `active_turn` 更新为 `turn-101`。
4. 提交事务成功后，内存中的 `ActiveTurn` 才设置为 `turn-101`。
5. 广播 `TurnStarted` 和 `MessageCommitted(user)` 事件。

此时数据库变为：

```text
sessions: sess-A.active_turn = turn-101
messages: (501, sess-A, user, "帮我总结今天的日志", generation=0)
turns:    (turn-101, running, generation=0)
```

事件顺序应为：

```text
seq=1  TurnStarted { turn_id: turn-101, generation: 0 }
seq=2  MessageCommitted { message_id: 501, role: user }
```

先提交数据库，再发送 `MessageCommitted`，这样订阅者收到事件后立刻查询数据库不会读到“幽灵消息”。

### 18.3 Worker 生成 assistant 输出

Actor 启动 Worker，并把 `turn-101` 与取消令牌交给它。Worker 只负责调用 Provider 和发送 `WorkerEvent`，不直接访问 Store：

```text
WorkerEvent::Delta("今天")
WorkerEvent::Delta("的日志")
WorkerEvent::Delta("显示构建成功。")
WorkerEvent::FinalText("今天的日志显示构建成功。")
```

Actor 收到每个 Delta 后广播：

```text
seq=3  MessageDelta { turn_id: turn-101, text: "今天" }
seq=4  MessageDelta { turn_id: turn-101, text: "的日志" }
seq=5  MessageDelta { turn_id: turn-101, text: "显示构建成功。" }
```

Delta 是瞬时事件，不应该每次都插入 `messages` 表；UI 将它们按 `turn_id` 拼接为临时气泡。若客户端中途断线，可以通过 `events_since` 补事件，或在收到终态后重新读取数据库。

### 18.4 FinalText 收口

收到 `FinalText` 后，Actor 执行 `Store::complete_turn` 事务：

1. 插入 assistant 消息 `message_id = 502`，内容为完整文本；
2. 将 `turn-101` 状态改为 `completed`；
3. 清空 `sessions.active_turn`；
4. 提交事务。

事务提交成功后再广播：

```text
seq=6  MessageCommitted { message_id: 502, role: assistant }
seq=7  TurnCompleted { turn_id: turn-101 }
```

最终数据库状态：

```text
sessions: sess-A.active_turn = NULL
messages:
  501 user      帮我总结今天的日志
  502 assistant 今天的日志显示构建成功。
turns: (turn-101, completed, generation=0)
```

## 19. 取消和失败：消息与事件如何对应

### 19.1 用户主动中断

用户在 Delta 输出到一半时点击停止：

```text
客户端 ──Interrupt──> Actor
                         ├─取消 token
                         ├─abort Worker
                         └─Store::interrupt_turn
```

假设已经产生了 `"今天的"`，但还没有完整答案。`interrupt_turn` 只结束 running turn、清空 `active_turn`，不会伪造 assistant 消息：

```text
seq=8  TurnInterrupted { turn_id: turn-101, reason: user }
```

数据库仍只有 user 消息 501，turn 状态为 `interrupted`。下一次提交会创建新的 turn，而不是复用 `turn-101`。

### 19.2 Provider 或 Worker 失败

如果 Worker 返回错误：

```text
WorkerEvent::Failed("provider timeout")
```

Actor 调用 `Store::fail_turn`，将 turn 改为 `failed` 并释放 session 锁，然后广播：

```text
seq=8  TurnFailed { turn_id: turn-101, error: "provider timeout" }
```

同样不会插入空的 assistant 消息。错误详情放在 turn 记录或事件中，避免把失败文本伪装成模型回复。

## 20. 事件顺序、重放与消息查询

### 20.1 为什么事件必须带序列号

每个 Actor 的事件拥有单调递增 `seq`。客户端保存最后收到的序号，例如 `last_seq=5`。断线重连时请求 `events_since(5)`：

```text
seq=6  MessageCommitted(assistant)
seq=7  TurnCompleted
```

如果广播通道已经丢弃旧事件（`SubscriberLagged`），客户端不要猜测缺失内容，应重新执行：

1. 查询 `turns` 的终态；
2. 按 `session_id` 查询已提交消息；
3. 用数据库结果重建 UI；
4. 将 `last_seq` 更新为当前序号。

### 20.2 消息查询的可见性规则

- `MessageDelta` 只存在于内存/UI，不出现在历史消息查询中；
- running turn 尚未提交的 assistant 草稿不应被 `list_messages` 返回；
- `MessageCommitted` 之后才允许历史接口返回消息；
- system、压缩摘要等内部消息是否展示，由查询层按 role/type 过滤；
- generation 或分支过滤必须在 SQL 层完成，不能先取全量再由 UI 猜测。

因此，“当前屏幕看到的内容”和“重新打开会话读到的历史”可能短暂不同：前者包含 Delta，后者只包含已提交 Message。这是预期行为。

## 21. 两个会话并行时的流转

`sess-A` 正在生成时，用户又向 `sess-B` 发送消息：

```text
sess-A mailbox: SubmitPrompt ─> running turn-A ─> Delta...
sess-B mailbox: SubmitPrompt ─> running turn-B ─> Delta...
```

Supervisor 为每个 session 创建独立 Actor，因此：

- A 的 `active_turn` 不会阻塞 B；
- A 的事件不会被 B 的订阅者收到；
- 每个 Actor 的 Store 写入仍按自身 mailbox 串行化；
- 全局数据库连接可以共享，但事务边界必须按 session/turn 正确划分。

同一会话的两个 SubmitPrompt 则不同：第一个命令把 `active_turn` 设为 running，第二个命令在 Actor 内看到该状态并返回 `Busy`，不会插入第二条 user 消息。

## 22. 与 Python 实现的对应关系

```text
Python tui_gateway/server.py::_run_prompt_submit
  └─接收请求、检查 busy、启动 _start_inflight_turn

Rust SessionHandle::submit_prompt
  └─发送 SubmitPrompt 到 SessionActor mailbox

Python _emit("message.delta")
  └─Rust RuntimeEvent::MessageDelta

Python hermes_state.py 保存 user/assistant 消息
  └─Rust Store::begin_turn / complete_turn

Python AIAgent.interrupt()
  └─Rust CancellationToken + Worker abort + interrupt_turn
```

迁移时应保持一个关键不变量：Python 中任何会改变会话和消息状态的路径，在 Rust 中都必须经过 Actor；Provider、TUI、RPC 只能发送命令或消费事件，不能绕过 Actor 直接更新数据库。

## 23. 排查问题时的推荐日志

按以下字段关联一次完整请求：

```text
session_id=sess-A
turn_id=turn-101
message_id=501/502
generation=0
event_seq=1..7
```

看到 `TurnCompleted` 却查不到 assistant 消息，说明事件发送早于事务提交或收口事务失败；看到两个 `TurnStarted`，说明 busy 检查和 `begin_turn` 没有处于同一个 Actor 串行路径；看到 UI 有 Delta 但历史没有对应消息，则应先确认是否发生了 interrupt/failed，而不是立即判定数据库丢数据。

## 24. 会话创建、加载与恢复

### 24.1 首次创建会话

会话创建发生在提交 prompt 之前。调用方可以先显式创建，也可以由上层命令在发现会话不存在时创建：

```text
客户端
  │ session.create(profile=default)
  ▼
Store::create_session
  ├─INSERT sessions(session_id, profile, created_at, updated_at)
  └─提交事务
  ▼
Supervisor::get_or_start(session_id)
  └─启动唯一 SessionActor
```

创建会话只写 `sessions` 表，不创建 Turn，也不插入消息。只有第一次 `SubmitPrompt` 才会创建 generation=0 和 running Turn。

### 24.2 加载已有会话

`get_or_start` 的处理顺序如下：

1. 先在 Supervisor 的 map 中查找 actor；
2. 找到且 actor 仍运行，直接返回新的 Handle；
3. 找不到时打开对应 profile 的 Store；
4. 查询 `sessions`，确认 `session_id` 存在；
5. 检查是否存在未收口的 running Turn；
6. 启动 Actor，并把恢复结果放入初始状态；
7. 将 actor 放入 map 后返回 Handle。

同一 Session 的并发 `get_or_start` 必须只有一个调用真正完成启动。map 锁只能保护“检查和插入”，不能在锁内执行数据库 I/O 或等待 actor。

### 24.3 打开会话时发现 running Turn

进程异常退出可能留下：

```text
turn-101.status = running
sessions.active_turn = turn-101
```

新 Actor 不能把这个 Turn 当作仍在执行，因为原 Worker 已不存在。建议恢复策略为：

```text
启动 Actor
  ├─发现 running Turn
  ├─调用 Store::fail_turn(reason="worker lost after restart")
  ├─清空 sessions.active_turn
  └─发布 TurnRecovered/TurnFailed（按协议选定一种）
```

恢复操作必须幂等：两个进程同时尝试恢复同一个 Turn 时，只有第一个事务能把 `running` 改成终态，另一个得到“已收口”结果，不得重复写失败事件。

## 25. request_id 与幂等提交

`request_id` 用于把客户端请求、Actor 命令、Worker 事件和 RuntimeEvent 关联起来。它不应只停留在日志中，还要明确重复请求的语义。

### 25.1 重复 SubmitPrompt 示例

```text
req-1: SubmitPrompt("总结日志")
  → begin_turn 成功，插入 user message 501，返回 Accepted(turn-101)

网络超时，客户端重试：
req-1: SubmitPrompt("总结日志")
  → 查询 request_id=req-1 的处理记录
  → 返回原 Accepted(turn-101)
  → 不再插入 message 502，不再创建第二个 Turn
```

可以在 turns 或独立 request_records 表上建立唯一约束：

```text
UNIQUE(session_id, request_id)
```

如果当前阶段暂不增加表，至少要在 Actor 内保存最近请求映射，并在进程恢复后以数据库唯一约束作为最终防线。不能仅依赖“先检查再插入”，因为两个进程或未来的多入口 RPC 可能同时通过检查。

### 25.2 Interrupt 的幂等性

同一个 `request_id` 重复中断时：

```text
第一次 Interrupt(req-2)  → running -> interrupted，发布一次 TurnInterrupted
第二次 Interrupt(req-2)  → 返回已完成，不重复写事件
```

不同 request_id 的第二次 Interrupt，如果 Turn 已经是终态，应返回该终态或 `NoActiveTurn`，但不能再次修改消息和 Turn。

## 26. Mailbox 背压和容量管理

每个 SessionActor 使用固定容量的 mailbox（当前建议容量为 32）。容量限制是保护系统，而不是业务队列。

```text
发送命令
  ├─有空位 → try_send 成功，等待 actor 应答
  ├─已满   → 立即返回 MailboxFull
  └─已关闭 → 返回 MailboxClosed/ActorStopped
```

### 26.1 不同命令的处理建议

| 命令 | mailbox 满时建议 |
| --- | --- |
| SubmitPrompt | 返回错误，由客户端提示稍后重试 |
| Interrupt | 应优先保证送达；必要时为控制命令单独保留容量 |
| Close | 可使用关闭信号或独立 control channel，不能静默丢失 |

当前 4.3 只实现统一容量和 `MailboxFull`，没有隐式 FIFO 队列。后续如果加入 queued prompt，必须明确：队列上限、取消队列项、顺序、持久化和重启恢复规则。

### 26.2 背压示例

```text
容量 = 2
命令 1 SubmitPrompt → 入队
命令 2 SubmitPrompt → 入队
命令 3 SubmitPrompt → MailboxFull
```

`MailboxFull` 不等同于 `Busy`：

- `Busy` 表示 actor 已经接受并正在执行一个 Turn；
- `MailboxFull` 表示命令甚至没有进入 actor；
- `Busy` 通常是业务状态；
- `MailboxFull` 通常是瞬时容量状态。

## 27. 进程崩溃和重启恢复

### 27.1 崩溃时可能留下的状态

SQLite 事务具有原子性，因此只会出现以下两类合法状态：

```text
A. begin_turn 事务已提交
   user message + running turn + active_turn 都存在

B. begin_turn 事务未提交
   三者都不存在，Actor 尚未启动 Worker
```

不会出现“只有 user 消息、没有 Turn”的半提交状态。若 Worker 运行期间进程崩溃，则 A 会遗留 running Turn。

### 27.2 启动扫描

应用启动时可按 profile 执行恢复扫描：

```sql
SELECT turn_id, session_id
FROM turns
WHERE status = 'running';
```

对每条结果：

1. 验证 `sessions.active_turn` 是否指向该 Turn；
2. 将不可恢复的 Turn 标记为 `failed` 或 `interrupted`；
3. 写入一条恢复原因（如 `worker_lost`）；
4. 清空 `active_turn`；
5. 让用户可以重新提交 prompt。

恢复事件应包含 `recovered=true` 或明确 reason，便于 TUI/RPC 显示“上次任务因进程退出而结束”。

### 27.3 恢复期间的并发保护

恢复扫描与新 SubmitPrompt 不能交叉：

```text
恢复 Turn-A              新 SubmitPrompt
     │                         │
     ├─收口 running             ├─检查 active_turn
     ├─清空 active_turn         └─看到已清空后创建 Turn-B
     └─提交事务
```

实际实现应让恢复也通过 SessionActor 串行执行，或使用数据库条件更新（`WHERE status='running'`）保证只有一个恢复者成功。

## 28. 数据库表、操作和事件映射

| 操作 | `sessions` | `session_generations` | `turns` | `messages` | `daemon_events` |
| --- | --- | --- | --- | --- | --- |
| 创建会话 | 插入会话 | 无 | 无 | 无 | 可选 session.created |
| 创建 generation | 无 | 插入 generation | 无 | 无 | generation.created |
| `begin_turn` | 设置 `active_turn` | 校验 generation | 插入 running | 插入 user | turn.started、message.committed |
| Delta | 无 | 无 | 保持 running | 不写入 | 不写入 |
| `complete_turn` | 清空 `active_turn` | 保持不变 | completed | 插入 assistant | message.committed、turn.completed |
| `interrupt_turn` | 清空 `active_turn` | 保持不变 | interrupted | 不插入 assistant | turn.interrupted |
| `fail_turn` | 清空 `active_turn` | 保持不变 | failed | 不插入 assistant | turn.failed |
| 重启恢复 | 清理 `active_turn` | 保持不变 | running→failed/interrupted | 不新增模型消息 | turn.recovered、turn.failed |

关键原则是：事实型事件（消息提交、Turn 终态）必须和对应数据库写入处于同一事务，或至少在事务提交后再广播；Delta 只能作为运行时事件。

## 29. 不同订阅者的职责

### 29.1 TUI 订阅者

TUI 需要低延迟显示：

```text
TurnStarted → MessageDelta* → FinalMessagePersisted → TurnCompleted
```

它可以丢失 Delta，但不能丢失终态。收到 lag 后应清空临时草稿，调用历史查询或 `events_since` 重建。

### 29.2 RPC/桌面客户端

RPC 客户端需要：

- 保存 `last_sequence`；
- 为每个请求保存 `request_id`；
- 断线后补读持久化事件；
- 不能把本地重试当作新的用户消息；
- 收到 `ActorStopped` 后重新获取 Handle。

### 29.3 日志和监控订阅者

日志系统通常只关心：

```text
turn.started / turn.completed / turn.failed / turn.interrupted
```

不应记录每个 Delta 的完整文本，避免日志膨胀和敏感内容泄漏。可记录长度、耗时、token 数等统计信息。

### 29.4 历史查询

历史页面不应依赖实时广播，而应直接读取 Store：

```text
list_messages(session_id, generation, limit, offset)
```

这样即使客户端从未订阅过事件，也能得到一致的已提交历史。

## 30. 4.4 Provider 接入边界

4.4 接入真实 Provider 时，Provider 仍然不能持有 Store。它只把外部协议转换为 Actor 已定义的模型无关事件：

```text
OpenAI SSE data.delta       → WorkerEvent::TextDelta(text)
OpenAI finish_reason=stop   → WorkerEvent::FinalText(full_text)
HTTP 4xx/5xx 或解析错误     → WorkerEvent::Failed(error)
CancellationToken 被取消     → WorkerEvent::Cancelled
连接 EOF（无 finish）         → WorkerEvent::Failed(incomplete_stream)
```

完整边界如下：

```text
Provider：HTTP/SSE、重试、协议解析、token 统计
       │
       ▼ WorkerEvent
SessionActor：状态迁移、取消竞争、Store 事务、RuntimeEvent
       │
       ▼ RuntimeEvent
TUI/RPC：显示、重连、用户交互
```

Provider 不得直接发布 `TurnCompleted`，也不得直接插入 assistant 消息。只有 Actor 在 `FinalText` 收到后完成 `complete_turn`，才能产生完成事件。

如果未来加入工具调用，流转扩展为：

```text
TextDelta → ToolCallStarted → ToolResult → TextDelta → FinalText
```

工具结果同样必须通过 WorkerEvent 回到 Actor，由 Actor 决定是否持久化和发布事件。

## 31. 补充后的验收问题

实现或评审时，可以用以下问题验证整条链路：

1. 会话不存在时，谁创建 `sessions` 行？
2. 重复 `request_id` 会不会产生两条 user 消息？
3. `Busy` 和 `MailboxFull` 是否能被客户端区分？
4. 进程重启后 running Turn 谁负责收口？
5. 事件发送时，对应消息是否已经能从 Store 读到？
6. Delta 丢失时，客户端是否能通过数据库恢复？
7. Provider 是否完全不依赖 Store？
8. 两个 profile 使用相同 SessionId 时，是否仍然隔离？

只要其中任一问题没有明确答案，Actor、会话、消息和事件之间的边界就还不完整。
