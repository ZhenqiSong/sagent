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
