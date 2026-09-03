# Sagent Rust 第四阶段 4.3 计划：SessionActor、并发边界与取消

作者：SongZQ  
状态：实施计划  
前置条件：4.1 已完成纯领域状态机、PromptSnapshot 和 transcript 不变量；4.2 已完成 schema v3、generation/turn/message/event 的原子 Store API，并通过 workspace 质量门禁。

## 1. 目标

4.3 创建 sagent-runtime，让每个 Session 都由一个 SessionActor 串行处理命令。该 actor 是唯一允许调用 sagent-store 写入 transcript、Turn 和 daemon event 的组件。

    外部调用者（后续 RPC / TUI）
                    │ SessionCommand
                    ▼
    SessionSupervisor ── 每个 SessionId 一个有界 mailbox ──► SessionActor
                                                              │
                                                              ├─ 唯一 Store 写入者
                                                              ├─ 驱动 4.1 transition
                                                              ├─ 管理当前 Turn 的 CancellationToken
                                                              └─ 发布运行时事件

本阶段完成后，可用测试替身驱动“接受 prompt → 持久化 user/running turn → 完成或中断”的闭环，并证明并发请求不会交叉写同一 transcript。真实 OpenAI SSE、工具、approval、公开 RPC 方法与 TUI 留给 4.4 至 4.7。

## 2. 范围与非目标

本阶段实现：

- 新建 sagent-runtime crate，加入 workspace；
- SessionSupervisor、SessionHandle、SessionActor 和有界 mailbox；
- SubmitPrompt、busy 拒绝、Interrupt、Close 的最小语义；
- 当前 Turn 的取消和受监管 worker task；
- 4.2 generation、begin_turn、complete_turn、interrupt_turn、fail_turn 的 actor 调用边界；
- 可订阅的 RuntimeEvent，及完整的并发/取消/DB 失败测试。

本阶段不实现：

- sagent-provider、真实 OpenAI-compatible HTTP/SSE；属于 4.4；
- sagent-tools、read_file、terminal、approval 执行；属于 4.5；
- sagent-rpc 新方法和 ratatui；属于 4.6/4.7；
- compression、fork、retry 的 runtime generation 迁移；
- 通过请求参数传入任意 home、profile 或 SQLite 路径；
- 全局 Mutex<Store>、长事务、无归属 tokio::spawn。

## 3. Python 参考与取舍

| Python 位置 | 需要保留的行为 | Rust 4.3 落点 | 不照搬的部分 |
| --- | --- | --- | --- |
| tui_gateway/server.py:_start_inflight_turn（约 8417） | 接受 prompt 时建立明确的 in-flight turn | actor 内 ActiveTurn | 每 session 可变 dict |
| tui_gateway/server.py:_interrupt_busy_session（约 8794） | 中断不能阻塞 RPC reader；重复中断不能泄漏后台任务 | CancellationToken + actor 所有的 JoinHandle | Python thread + history_lock |
| tui_gateway/server.py:_drain_queued_prompt（约 8927） | busy 语义、优先级必须明确 | 4.3 固定 busy 拒绝；后续再单独引入 FIFO/steer | queued_prompt(s) 隐式字段 |
| tui_gateway/server.py:_run_prompt_submit（约 11356） | 接受、执行、结束必须成对收尾 | actor ActiveTurn 生命周期 | gateway callback 与业务混合 |
| run_agent.py:AIAgent.run_conversation（约 8493） | 一次回合中按顺序解释模型、工具、取消和最终结果 | actor 驱动 4.1 transition；worker 回传输入 | 巨型 AIAgent 类 |
| hermes_state.py:SessionDB.append_message（约 10498） | transcript、session 计数、活动时间使用短事务 | 只调用 4.2 repository | runtime 拼 SQL |

Python 支持 queue、steer、interrupt 多种 busy 策略。为了先锁定一致性，4.3 的唯一策略是：Session 有 active Turn 时，新 SubmitPrompt 返回 Busy，且不写数据库；Interrupt 仍可进入 mailbox 并取消当前 Turn。FIFO queue 和 steer 后续以独立需求加入。

## 4. 不可破坏的约束

1. 同一 SessionId 的状态变迁和 Store 写入只能发生在一个 actor task。
2. 不同 Session 可以并行，Supervisor 不能用全局锁串行所有会话。
3. user message、running turn、turn.started 与 message.committed 必须先由 Store 提交，才发布 UserMessagePersisted。
4. provider/tool worker 不得持有 Store，也不得直接执行 SQL；只能向 actor 发送 WorkerEvent。
5. actor 等待 worker、JoinHandle 或 channel 时不持有 SQLite transaction。
6. Interrupt 必须取消 active token，并把 running Turn 持久化为 interrupted；不得插入空 assistant message。
7. Close 必须收尾已进入 running 的 Turn，之后才退出。
8. actor 退出后 Supervisor 必须清理 stale handle，旧 handle 返回 ActorStopped。
9. ModelTextDelta 是瞬态 UI 事件，不能写入 daemon_events；完成事实必须先提交后通知。

## 5. crate、模块与依赖

新增目录：

    crates/sagent-runtime/
    ├── Cargo.toml
    └── src/
        ├── lib.rs
        ├── error.rs
        ├── supervisor.rs
        ├── actor.rs
        ├── active_turn.rs
        ├── input.rs
        ├── event.rs
        └── test_support.rs

初始依赖：

    sagent-agent = { path = "../sagent-agent" }
    sagent-store = { path = "../sagent-store" }
    sagent-types = { path = "../sagent-types" }
    thiserror.workspace = true
    serde.workspace = true
    serde_json.workspace = true
    tokio = { version = "1", features = ["rt", "macros", "sync", "time"] }
    tokio-util = { version = "0.7", features = ["rt"] }

不要提前引入 reqwest、tokio::process 或 provider/tool crate。若测试确实要求多线程运行时，再最小化启用 rt-multi-thread。

依赖方向：

    sagent-types ← sagent-agent / sagent-store
    sagent-agent + sagent-store + sagent-config
                      ↑
                 sagent-runtime
                      ↑
          sagent-rpc / sagent-tui（后续）

Store 由 actor 独占。它不跨 await 暴露，也不由 SessionHandle、worker、RPC 或 TUI 取得。

## 6. 类型与公开 API

建议的最小公开边界：

    pub struct SessionSupervisor { /* SessionId -> ManagedSession */ }

    pub struct SessionHandle {
        session_id: SessionId,
        command_tx: mpsc::Sender<ActorInput>,
        events: broadcast::Sender<RuntimeEvent>,
    }

    impl SessionSupervisor {
        pub async fn get_or_start(&self, session_id: SessionId)
            -> Result<SessionHandle, RuntimeError>;
        pub async fn remove(&self, session_id: &SessionId)
            -> Result<(), RuntimeError>;
    }

    impl SessionHandle {
        pub async fn submit(&self, request_id: RequestId, input: UserInput)
            -> Result<SubmitReceipt, RuntimeError>;
        pub async fn interrupt(&self, request_id: RequestId)
            -> Result<(), RuntimeError>;
        pub async fn close(&self) -> Result<(), RuntimeError>;
        pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent>;
    }

Handle 只负责把命令投递到 mailbox，并等待“accepted/busy”的一次应答；它不等待完整模型回合，也不暴露 Store。

actor 私有输入：

    enum ActorInput {
        Command {
            command: SessionCommand,
            reply_to: oneshot::Sender<Result<CommandReply, RuntimeError>>,
        },
        Worker(WorkerEvent),
        WorkerExited {
            turn_id: TurnId,
            result: Result<(), WorkerFailure>,
        },
    }

WorkerEvent 先是封闭的模型无关集合：TextDelta、FinalText、Failed、Cancelled。4.3 的 fake worker 用它驱动测试；4.4 的 provider adapter 以后也只产生相同事件，不能泄露 SSE 或 provider 专属 JSON。

RuntimeError 至少包含 Busy、MailboxFull、MailboxClosed、NoActiveTurn、Persistence、ActorStopped、InvalidLifecycle 和 RequiresTransition。Store 错误转换为 Persistence，并保留可读原因，但不向上暴露 SQLite connection。

## 7. actor 状态与持久化映射

    struct SessionActor {
        session_id: SessionId,
        store: Store,
        command_rx: mpsc::Receiver<ActorInput>,
        event_tx: broadcast::Sender<RuntimeEvent>,
        active: Option<ActiveTurn>,
        generation: u64,
    }

    struct ActiveTurn {
        turn_id: TurnId,
        generation: u64,
        cancellation: CancellationToken,
        state: TurnState,
        task: Option<JoinHandle<()>>,
    }

普通会话只确保 generation=0 存在并复用；不能每个 Turn 新建 generation。后续 compression 才可显式创建 generation=1、2 等。

### submit 的精确顺序

    SubmitPrompt
      ├─ active != None → reply Busy；不写 user message
      ├─ 构造并校验稳定 PromptSnapshot
      ├─ 确保 generation=0 存在；不存在则 create_generation
      ├─ 生成 TurnId，调用 Store::begin_turn
      │    原子写：user message + running turn + started/message events
      ├─ 设置 ActiveTurn
      ├─ 发布 PromptAccepted、UserMessagePersisted
      └─ 启动受监管 fake worker

create_generation 或 begin_turn 出错时，actor 产生 TurnFailure::Persistence，不设置 active，不启动 worker，也不发布误导性的 accepted 事件。

### worker 回传的精确顺序

- TextDelta：仅发布 RuntimeEvent::ModelTextDelta，不写 Store。
- FinalText：先调用 Store::complete_turn；成功后再发布 FinalMessagePersisted、Completed。
- Failed：调用 Store::fail_turn；成功后发布 Failed。
- Cancelled：调用 Store::interrupt_turn；成功后发布 Interrupted。
- 迟到事件：turn_id 不是 active Turn，或 token 已取消时，直接忽略；绝不修改新 Turn。

### interrupt 和 close

    1. actor 检查 active；无 active 则回复 NoActiveTurn，且不写库
    2. cancellation.cancel()
    3. 停止或等待 actor 所有的 worker task
    4. 对仍为 running 的 Turn 调用 Store::interrupt_turn
    5. 仅在持久化成功后发布 Interrupted / OutcomePersisted
    6. 清空 active，状态回到 Idle

FinalText 和 Interrupt 同时抵达时，mailbox 中先被 actor 处理的 terminal 输入获胜；之后另一输入因 Turn 已不再 running 而没有副作用。不能让两个任务竞态调用 Store。

Close 复用相同收尾逻辑，随后关闭 receiver；Supervisor 在收到 actor 结束通知后才删除 map 条目。

## 8. 逐步执行计划

### 步骤 0：锁定基线与 API

1. 运行 cargo fmt --check、cargo test --workspace --offline；
2. 记录 schema v3 和现有测试数；
3. 阅读 sagent-agent 的 command.rs、event.rs、transition.rs、prompt.rs；
4. 阅读 sagent-store 的 turn.rs、event.rs；
5. 确认 workspace 尚无 sagent-runtime，且本步骤不改 Store schema。

完成条件：后续回归可明确区分为 runtime 改动，而不是 4.2 遗留问题。

### 步骤 1：创建 crate 和错误边界

1. 新建 crate，加入根 Cargo.toml members；
2. 添加上述最小依赖和 RuntimeError；
3. lib.rs 仅导出 Supervisor、Handle、RuntimeEvent、错误及必要 DTO；
4. 为错误显示、offline 构建和依赖方向写测试。

完成条件：agent/store 不反向依赖 runtime。

### 步骤 2：定义输入、事件和 fake worker

1. 实现 ActorInput、WorkerEvent、RuntimeEvent；
2. 所有 runtime event 带 session_id、turn_id 和必要时 request_id；
3. 编写 test-only fake worker，可按脚本发送 delta/final/failure，并响应 CancellationToken；
4. 写 JSON round-trip、turn/session 归属和 cancellation 等单元测试。

完成条件：无网络、无工具的测试可模拟一轮模型输出。

### 步骤 3：实现 SessionActor 的 submit

1. actor 独占 Store；
2. 收到 SubmitPrompt 先检查 active；busy 立即返回；
3. 构造稳定 PromptSnapshot，确保 generation=0 存在；与已存 generation hash 不一致时返回 RequiresTransition；
4. 调用 Store::begin_turn；成功后才设置 ActiveTurn；
5. 确认 Store 已提交后才发布 PromptAccepted/UserMessagePersisted；
6. 由 actor 驱动 4.1 transition；非法迁移返回错误，不能 panic。

数据库断言：accepted prompt 只产生一条 user message、一条 running turn、turn.started 与 message.committed；busy 的第二条 prompt 不产生任何行。

### 步骤 4：实现 Supervisor、mailbox 与生命周期

1. 维护 SessionId 到 ManagedSession 的映射；
2. get_or_start 的 map 锁只覆盖检查/插入，不能在锁内 await；
3. mailbox 设置固定容量，例如 32；满时返回 MailboxFull；
4. 同 Session 并发 get_or_start 必须只启动一个 actor；
5. actor 退出时通知 Supervisor 删除 stale map 条目；
6. remove/close 发送 Close、等待 actor 结束、再删除条目。

测试：100 个并发 get_or_start 只创建一个 actor；不同 Session 的慢 fake worker 可真正并行。

### 步骤 5：实现取消、竞争与 task 监管

1. 每个 ActiveTurn 创建 CancellationToken，并只把子 token 给 worker；
2. Interrupt 通过 mailbox 进入 actor，取消 token、结束 worker、持久化 interrupted；
3. worker JoinHandle 由 actor 所有，禁止裸 tokio::spawn；
4. 首个 FinalText、Failed 或 Interrupt 获胜，后续 terminal 输入无副作用；
5. worker panic/JoinError 映射为受控 TurnFailure，再调用 fail_turn；
6. Close 复用 interrupt 收尾，退出时没有无主 task。

参考 tui_gateway/server.py:_interrupt_busy_session 的“不阻塞 reader、不重复创建中断 worker”原则；Rust 用 cancellation 和 task owner 实现。

测试：submit 后立即 interrupt；final 与 interrupt 并发；多次 interrupt；worker panic；close during active turn。

### 步骤 6：实现事件订阅和持久化顺序测试

1. Handle 提供 broadcast 订阅，但订阅者不能读取 actor 可变状态；
2. 慢订阅者漏掉 transient delta 时返回 lag 标记；4.6 再用 events_since 补读持久化事实；
3. 断言 user/final/tool 完成类 RuntimeEvent 出现前，对应 Store 行已经可读；
4. 断言 ModelTextDelta 永远不进入 daemon_events。

完成条件：UI 断线丢 delta 不影响数据库恢复；任何客户端都不会先看到完成、后发现数据库没有 final message。

### 步骤 7：runtime 集成测试

建议文件：

    crates/sagent-runtime/tests/session_actor.rs
    crates/sagent-runtime/tests/supervisor.rs

必须覆盖：

1. create session → actor → submit → fake final → reopen Store，可恢复 user/assistant/turn/event；
2. 同 Session 并发 submit：一个 accepted，一个 Busy，绝无相邻 user message；
3. 不同 Session 同时运行：可并行，事件和 DB 完全隔离；
4. begin_turn 触发 DB 失败：无 active、无 worker、无 accepted event；
5. delta 中 interrupt：最终 interrupted，且无 assistant 空消息；
6. final 与 interrupt 竞态：唯一终态和唯一终态 event；
7. active turn 上 Close：收尾后 Supervisor 不再保存 handle；
8. actor 意外退出：新 get_or_start 得到新 actor，旧 handle 返回 ActorStopped；
9. 两个 profile DB 的 actor 数据隔离。

### 步骤 8：门禁与提交边界

建议提交：

1. feat(runtime): add session actor crate and lifecycle errors
2. feat(runtime): serialize session commands through supervisor mailbox
3. feat(runtime): persist submitted turns through single writer
4. feat(runtime): cancel active turns and supervise worker tasks
5. test(runtime): cover actor concurrency, persistence and cancellation races

每个提交：

    cargo fmt --check
    cargo test -p sagent-runtime --offline
    cargo clippy -p sagent-runtime --all-targets --offline -- -D warnings

最后：

    cargo test --workspace --offline
    cargo clippy --workspace --all-targets --offline -- -D warnings

## 9. 验收清单

- [ ] 存在独立 sagent-runtime crate，且依赖方向正确；
- [ ] 同一 Session 永远只有一个 actor，mailbox 有容量上限；
- [ ] runtime 是唯一 Store 写入者；Handle、worker、RPC/TUI 都不能直接写 Store；
- [ ] busy submit 被拒绝，不产生 user message、Turn 或 event；
- [ ] accepted submit 已原子持久化 user/running Turn/started event；
- [ ] 不同 Session 可并行，取消 token、Turn、event 不泄漏；
- [ ] interrupt/close 取消 worker、持久化 interrupted，且没有伪造 assistant final；
- [ ] final/failure/interrupt 竞争时恰有一个 terminal outcome；
- [ ] actor crash/close 后没有 stale handle；
- [ ] delta 不持久化，完成事实始终先提交后通知；
- [ ] runtime 集成测试与 workspace fmt/test/clippy 均通过。

## 10. 下一步

4.3 完成后，系统拥有正确的会话串行化、持久化与取消边界，但 fake worker 还不能实际对话。4.4 将接入 mock 和 OpenAI-compatible provider；SSE 的 delta、finish、EOF、错误和取消都只能通过本计划定义的 WorkerEvent 回到 actor。

## 步骤 0 执行记录

执行日期：2026-09-03  
状态：已完成

### 基线验证

- cargo fmt --check：通过；
- cargo test --workspace --offline：通过，共 137 个测试通过，0 个失败；
- cargo clippy --workspace --all-targets --offline -- -D warnings：通过，无警告。

### API 与依赖核对

- 当前 sagent-store 的 SCHEMA_VERSION 为 3；
- 已确认 sagent-runtime 目录不存在，根 workspace 尚未注册该 crate；
- 已核对 sagent-agent 的 SessionCommand、TurnState、TurnEvent、PromptSnapshot；
- 已核对 sagent-store 的 create_generation、begin_turn、complete_turn、interrupt_turn、fail_turn、events_since；
- 本步骤未修改 Store schema、既有 crate 源码或 Hermes 数据库 fixture。

步骤 0 的结论：当前代码库具备进入 4.3 步骤 1 的稳定基线；后续实现应新增 sagent-runtime，由 actor 调用上述 Store API，而不是修改 Store 的持久化职责。

## 步骤 1 执行记录

执行日期：2026-09-03  
状态：已完成

已完成：

- 新增 crates/sagent-runtime/Cargo.toml，并注册到根 workspace；
- 新增 runtime 的 lib.rs 和 error.rs；
- 定义 RuntimeError：Busy、MailboxFull、MailboxClosed、NoActiveTurn、ActorStopped、Persistence、InvalidLifecycle、RequiresTransition、WorkerFailed；
- 暂时只导出 SessionSupervisor、SessionHandle 和 RuntimeError 骨架，不暴露 Store、SQLite 连接或 actor 内部状态；
- 配置 sagent-agent、sagent-store、sagent-types、serde、thiserror、tokio 和 tokio-util 的最小依赖；
- 添加错误上下文、持久化原因和生命周期错误互异性测试。

验证结果：

- cargo fmt --check：通过；
- cargo check -p sagent-runtime --offline：通过；
- cargo test -p sagent-runtime --offline：3 个测试通过，0 个失败；
- cargo clippy -p sagent-runtime --all-targets --offline -- -D warnings：通过。

本步骤没有启动 actor、创建数据库写入逻辑，也没有接入 Provider、Tools、RPC 或 TUI。

## 步骤 2 执行记录

执行日期：2026-09-03  
状态：已完成

已完成：

- 新增 runtime 内部输入模块 input.rs；
- 定义 ActorInput、CommandReply、WorkerEvent 和 WorkerFailure；
- 新增对外 RuntimeEvent 与 RuntimeEventKind；
- RuntimeEvent 统一携带 session_id、可选 turn_id 和 request_id；
- 采用扁平 JSON 事件结构，事件类型使用 snake_case；
- 新增 test-only fake worker，支持 Delta、Final、Fail 和 WaitForCancel 脚本；
- fake worker 通过 CancellationToken 感知取消，并通过 ActorInput 回传 Cancelled；
- worker 不直接访问 Store，不包含 HTTP、SSE 或 provider 专属类型；
- 增加事件 JSON round-trip、事件类型名称、worker 顺序、失败和取消测试。

验证结果：

- cargo fmt：通过；
- cargo test -p sagent-runtime --offline：8 个测试通过，0 个失败；
- cargo clippy -p sagent-runtime --all-targets --offline -- -D warnings：通过。

本步骤仍未实现 SessionActor 主循环、Supervisor、真实 Provider、工具执行或 RPC/TUI 接入。
