# Sagent Rust 第四阶段 4.2 计划：Turn、Generation 与事件持久化

作者：SongZQ  
状态：实施计划  
前置条件：4.1 已完成 `sagent-agent` 的 Turn 状态机、事件、`PromptSnapshot` 与 transcript 不变量；现有 `sagent-store` schema 为 v2。

## 步骤 0 执行记录

执行日期：2026-09-02  
状态：已完成

```text
cargo fmt --check                                      ✅
cargo test -p sagent-types -p sagent-store --offline   ✅ 49 tests passed
cargo clippy -p sagent-types -p sagent-store          ✅
```

当前基线：`sagent-store` schema version 为 2；Store 现有 39 个测试、`sagent-types` 现有 10 个测试全部通过。后续 4.2 migration 必须保持这些测试继续通过。

## 步骤 1 执行记录

执行日期：2026-09-02  
状态：已完成

已新增：

- `sagent-types::PersistedTurnStatus`：`running/completed/interrupted/failed`；
- `sagent-types::EventSequence`：非负、可排序、透明 JSON number；
- `sagent-types::TurnTypeError`：拒绝负事件序号；
- `crates/sagent-types/src/turn.rs` 模块及根导出；
- `sagent-types` 对 `thiserror` 的依赖。

验证结果：`sagent-types` 13 个测试通过，Clippy（`-D warnings`）通过；未修改 SQLite schema。

## 1. 目标与边界

4.2 的目标是让一个 Turn 在 SQLite 中留下可恢复、可审计且事务一致的结果，为后续 `SessionActor`、RPC event replay 和 TUI 重连提供唯一事实来源。

本步骤只扩展 `sagent-store`，不启动 Tokio、不调用模型、不执行工具、不增加 JSON-RPC 方法、不实现 TUI。完成后仍然不能聊天；它只保证 Runtime 未来有正确的写入原语可用。

```text
4.1 纯领域对象
  TurnState / TurnEvent / PromptSnapshot
              ↓
4.2 Store repository（本步骤）
  generation / turn / message / daemon event 的短事务
              ↓
4.3 SessionActor
  成为唯一调用 repository 的写入者
```

### 1.1 必须保持的约束

- 只迁移 Sagent 自己创建的 `state.db`；Hermes 的 `state.db` 继续只能通过 `Store::open_readonly` 读取，绝不自动迁移。
- migration 只能新增表、索引和数据，不能删除或重建 `sessions`、`messages`、FTS 表及触发器。
- 模型请求、SSE 流读取、工具执行和 approval 等待期间不持有 SQLite transaction。
- user 消息、最终 assistant 消息、最终 tool 结果必须在向客户端发送“完成”事件之前提交事务。
- token delta、spinner、局部流式渲染不进入 `daemon_events`；它们不是恢复时的事实。
- Store 不依赖 `sagent-agent`。4.1 的瞬态 `TurnState` 属于领域层；Store 只保存可恢复的持久化结果。

## 2. Python 参考与应保留的行为

| Python 文件/符号 | 应保留的行为 | Rust 4.2 落点 |
| --- | --- | --- |
| `hermes_state.py:4917` 的写事务封装 | 短写事务；失败时整体回滚 | 每个 repository 操作只包裹 SQL 写入 |
| `hermes_state.py:10347` 附近的 append fence | transcript 写入必须受单会话写者保护 | 4.2 预留一致 API；4.3 Actor 才提供唯一写者 |
| `hermes_state.py:10498` 的 `append_message` | 消息插入与 session 计数/活跃时间一起提交 | 抽取复用的 transaction 内消息插入 helper |
| `hermes_state.py:10635`、`10653` | transcript 写失败时不得留下半提交记录 | begin/finalize/tool-result 均使用单事务 |
| `run_agent.py:2020`、`2400` 附近 | turn 的消息批量落盘；最终结果可恢复 | 创建 turn、提交最终消息与 outcome 的 repository API |
| `run_agent.py:2055`、`2065` 附近 | 不生成破坏 role 顺序的伪造消息 | Store 只写 Runtime 已通过 4.1 transcript 校验的消息 |
| `tui_gateway/server.py` 的 `prompt.submit`、interrupt/reconnect 流程 | 已完成事实可在重连后读取；流式 delta 不重放 | `daemon_events` + `events_since` |
| `agent/prompt_cache_scope.py` | compression lineage 与普通 session/fork 隔离 | generation 表预留 generation；4.2 不实现压缩或 lineage 改写 |

不要复制 Python 的全局锁、租约重试、压缩、分支、gateway callback 或巨型 `AIAgent`。这些分别属于 4.3+ Runtime、后续 compression 和 transport 层。

## 3. 数据模型决策

### 3.1 状态的两个层次

4.1 的 `TurnState` 包含 `Prompting`、`AwaitingModel`、`RunningTool` 等瞬态状态；Store 不应为此依赖 `sagent-agent`。

在 `sagent-types` 新增持久化结果枚举：

```rust
enum PersistedTurnStatus {
    Running,
    Completed,
    Interrupted,
    Failed,
}
```

Runtime 在 4.3 负责映射：

```text
Prompting / AwaitingModel / RunningTool / AwaitingApproval -> Running
Completed                                                   -> Completed
Interrupted                                                 -> Interrupted
Failed                                                      -> Failed
```

这样 `sagent-store` 只依赖 `sagent-types`，保持既有依赖方向。

### 3.2 migration v3 的表

将 `SCHEMA_VERSION` 从 `2` 升至 `3`。全部使用当前数据库已有的 RFC 3339 TEXT 时间格式。

```sql
CREATE TABLE session_generations (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    generation INTEGER NOT NULL CHECK (generation >= 0),
    system_hash TEXT NOT NULL,
    tool_schema_hash TEXT NOT NULL,
    model_id TEXT NOT NULL,
    profile_revision TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (session_id, generation)
);

CREATE TABLE turns (
    turn_id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    generation INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'completed', 'interrupted', 'failed')),
    user_message_id INTEGER REFERENCES messages(id),
    assistant_message_id INTEGER REFERENCES messages(id),
    started_at TEXT NOT NULL,
    completed_at TEXT,
    outcome_json TEXT,
    FOREIGN KEY (session_id, generation)
      REFERENCES session_generations(session_id, generation)
);

CREATE TABLE daemon_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_id TEXT REFERENCES turns(turn_id) ON DELETE SET NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_turns_session_started
ON turns(session_id, started_at);

CREATE INDEX idx_daemon_events_session_sequence
ON daemon_events(session_id, sequence);
```

说明：

- `generation=0` 是普通会话的初始 generation。4.2 不创建 compression generation；4.4+ 才可新增 generation。
- `system_hash`、`tool_schema_hash` 先保存 4.1 产生的文本 hash；工具 schema 尚未实现时允许写入明确的稳定占位 hash，而不是 NULL 或随机值。
- `outcome_json` 保存已完成结果的结构化摘要，例如 `{"kind":"completed"}`、`{"kind":"interrupted","reason":"user_request"}`。不要把完整 token delta 或 provider 原始响应放进去。
- `daemon_events.sequence` 是数据库分配的单调序号，不按 session 重置；重连查询以 `session_id + sequence > after` 过滤。
- `daemon_events.turn_id` 允许 NULL，给未来 session 级事件预留；4.2 新写入的 turn event 都应带 turn ID。

## 4. 新增 Rust 类型与模块

### 4.2.1 `sagent-types`

文件：

```text
crates/sagent-types/src/turn.rs
crates/sagent-types/src/ids.rs
crates/sagent-types/src/lib.rs
```

新增：

```rust
pub struct EventSequence(i64);

pub enum PersistedTurnStatus {
    Running,
    Completed,
    Interrupted,
    Failed,
}
```

要求：

- `EventSequence` 不接受负值；提供 `new`, `get`，并用透明 JSON number 序列化。
- `PersistedTurnStatus` 使用 `snake_case` JSON 文本，与 SQL CHECK 值保持一致。
- 为负 sequence、JSON round-trip、状态字符串稳定性写单元测试。

`TurnId` 已在 4.1 中存在，必须继续作为 `turns.turn_id` 的 UUID 文本值；不要退化为数据库自增 ID。

### 4.2.2 `sagent-store`

建议新增模块：

```text
crates/sagent-store/src/turn.rs
crates/sagent-store/src/event.rs
```

并调整：

```text
crates/sagent-store/src/migration.rs
crates/sagent-store/src/write.rs
crates/sagent-store/src/lib.rs
```

公开 DTO 放在 Store 层：

```rust
pub struct NewGeneration { ... }
pub struct StartTurn { ... }
pub struct TurnCompletion { ... }
pub struct PersistedTurn { ... }
pub struct NewDaemonEvent { ... }
pub struct StoredDaemonEvent { ... }
pub struct EventQuery { session_id, after_sequence, limit }
```

`payload_json` 与 `outcome_json` 在写入前必须使用 `serde_json::Value` 或强类型 DTO 序列化，不接受调用方提供的任意 JSON 字符串。这样能保证数据合法，避免把无效 JSON 写进 event log。

## 5. 逐步执行计划

### 步骤 0：锁定现状与测试基线

目的：确认后续 schema 回归是由 4.2 造成，而非已有问题。

执行：

1. 运行 `cargo fmt --check`。
2. 运行 `cargo test -p sagent-types -p sagent-store --offline`。
3. 运行 `cargo clippy -p sagent-types -p sagent-store --all-targets --offline -- -D warnings`。
4. 记录当前 `SCHEMA_VERSION=2`，不修改 Hermes fixture。

参考：`crates/sagent-store/src/migration.rs`、`crates/sagent-store/src/lib.rs` 中现有 migration 回归测试。

完成条件：现有 Store、FTS、回退、恢复、重试测试均为绿。

### 步骤 1：先定义跨 crate 的持久化最小类型

目的：避免 Store 依赖 `sagent-agent` 的瞬态状态机。

执行：

1. 新建 `sagent-types/src/turn.rs`。
2. 定义 `PersistedTurnStatus` 和 `EventSequence`。
3. 在 `sagent-types/src/lib.rs` 导出类型。
4. 对 serde JSON 形状和非法 EventSequence 值写测试。

不要做：

- 不迁移 `TurnState` 出 `sagent-agent`；它仍是状态机内部模型。
- 不在本步骤定义 Provider、Tool 或 RPC DTO。

完成条件：Store 可引用所有持久化状态类型，且不新增对 `sagent-agent` 的 Cargo 依赖。

### 步骤 2：实现 v2 → v3 加性 migration

目的：在不影响 `sessions/messages/FTS` 的条件下创建三张新表。

执行：

1. 在 `migration.rs` 将 `SCHEMA_VERSION` 改为 `3`。
2. 新增 `Some(2)` 分支，在一个 migration transaction 内创建表和索引，再更新 `schema_version`。
3. 将当前新库初始化 SQL 改为直接创建 v3 完整 schema，避免新库先 v2 再 ALTER。
4. 保留 `Some(1)` 的 v1→v2 逻辑；随后在同一 migration 调用中继续执行 v2→v3。推荐把 migration 写成按版本逐步循环，而不是 `match` 中遗漏后续版本。
5. 不触碰 `messages_fts` 及三个 FTS trigger。

测试：

- 空数据库打开后有 v3、三张表和两个索引。
- v1 fixture 能升级至 v3，原 sessions/messages 可读。
- 新增 `historic_v2.sql` fixture，验证 v2→v3。
- 高于 v3 的版本继续拒绝。
- migration 故障回滚时 `schema_version` 不能提前变为 3。

参考：`crates/sagent-store/tests/fixtures/historic_v1.sql`、`crates/sagent-store/src/migration.rs`；Python 的 `hermes_state.py` 只参考“短事务 migration”原则。

完成条件：任何旧 Sagent 数据库升级后仍能通过 `get_messages_for_display`、`search_messages`、rewind/restore/retry 既有测试。

### 步骤 3：创建 generation 与开始 Turn 的原子 API

目的：保证普通 Turn 在持久化用户消息前已经绑定稳定 prompt/tool/model generation。

建议 API：

```rust
pub fn create_generation(&mut self, generation: &NewGeneration) -> Result<()>;

pub fn begin_turn(
    &mut self,
    turn: &StartTurn,
    user_message: &NewMessage,
    occurred_at: &str,
) -> Result<MessageId>;
```

`begin_turn` 必须在同一 transaction 内：

```text
验证 session 存在
验证 (session_id, generation) 存在
验证 turn_id 尚不存在
INSERT messages(user)
UPDATE sessions(message_count, last_activity_at, updated_at)
INSERT turns(status=running, user_message_id=...)
INSERT daemon_events(turn.started)
INSERT daemon_events(message.committed)
COMMIT
```

事件 payload 最小建议：

```json
{"message_id":101,"role":"user"}
```

不要在此方法写模型流式 delta，也不要预先插入空 assistant 消息。

测试：

- generation 缺失时整个 begin_turn 失败且没有 user message。
- 重复 `turn_id` 不产生第二条用户消息。
- 成功后 message count、turn user ID、两个 event sequence 都正确。
- 事件 sequence 单调递增。
- user 消息仍能被旧 CLI 的 session/message 查询读出。

参考：`sagent-store/src/write.rs` 的 `insert_message` 与 `append_message`；Python `hermes_state.py:10498` 的消息计数更新语义。

### 步骤 4：提交工具最终结果

目的：让后续 actor 能将可恢复的 tool 结果作为普通 `messages.role='tool'` 写入，而不是仅保存在内存。

建议 API：

```rust
pub fn commit_tool_result(
    &mut self,
    turn_id: &TurnId,
    message: &NewMessage,
    event: &NewDaemonEvent,
) -> Result<MessageId>;
```

事务规则：

```text
验证 turn 存在且 status=running
验证 message.session_id 与 turn.session_id 一致
INSERT tool message
UPDATE sessions 计数和时间
INSERT daemon_events(tool.completed / message.committed)
COMMIT
```

4.2 只保证持久化原子性；tool_call_id 是否存在、是否重复、是否还有 pending tool，由 4.1 transcript 和 4.3 actor 保证。Store 可以增加基础一致性检查，但不要在 SQL 层重写领域状态机。

测试：

- tool message 的 `tool_call_id`、`tool_name`、`display_metadata` 读回不变。
- session 不匹配或已结束 turn 时回滚。
- FTS 可搜索 tool 输出，但 display query 的现有隐藏规则不变。

### 步骤 5：原子完成、失败与中断 Turn

目的：落实“先持久化后完成”和“不产生半完成 final message”。

建议 API：

```rust
pub fn complete_turn(
    &mut self,
    turn_id: &TurnId,
    assistant_message: &NewMessage,
    outcome: &TurnOutcome,
    completed_at: &str,
) -> Result<MessageId>;

pub fn interrupt_turn(
    &mut self,
    turn_id: &TurnId,
    outcome: &TurnOutcome,
    completed_at: &str,
) -> Result<()>;

pub fn fail_turn(
    &mut self,
    turn_id: &TurnId,
    outcome: &TurnOutcome,
    completed_at: &str,
) -> Result<()>;
```

`complete_turn` 单事务：

```text
验证 turn=running
验证 assistant message 归属 session
INSERT assistant final message
UPDATE sessions message_count / last_activity_at / updated_at
UPDATE turns:
  status='completed', assistant_message_id=?, completed_at=?, outcome_json=?
INSERT daemon_events(message.committed)
INSERT daemon_events(turn.completed)
COMMIT
```

`interrupt_turn` 和 `fail_turn` 不写伪造 assistant 最终消息，只更新 turn outcome，并各插入 `turn.interrupted` 或 `turn.failed` 事件。

幂等性策略必须先定死：

```text
同一 terminal outcome 再次提交 -> 返回已完成的 PersistedTurn，不插入消息/事件
不同 terminal outcome 再次提交 -> 明确错误，不修改任何记录
running turn 才能写 final assistant message
```

测试：

- `complete_turn` 成功后 assistant ID、outcome、两条完成事件一致。
- 为 `daemon_events` 建临时 abort trigger 后执行 complete；断言 assistant 消息、turn status、message_count 都没有改变。
- 中断/失败不会新增 assistant message。
- 先 complete 再 interrupt 会稳定拒绝；重复 complete 不产生第二条 assistant 消息。
- 最终 assistant 消息可被现有 display/model 查询读取，FTS 不回归。

参考：`hermes_state.py:10635`、`10720` 的“关键 transcript 写失败必须回滚”原则；`run_agent.py` 的中断路径只持久化实际发生的消息。

### 步骤 6：实现 event replay 查询

目的：给 4.6 RPC 的 `session.events.since` 提供只读、分页且顺序稳定的 repository API。

建议 API：

```rust
pub fn events_since(&self, query: &EventQuery) -> Result<Vec<StoredDaemonEvent>>;
```

SQL 语义：

```sql
SELECT sequence, session_id, turn_id, event_type, payload_json, created_at
FROM daemon_events
WHERE session_id = ?
  AND sequence > ?
ORDER BY sequence ASC
LIMIT ?;
```

规则：

- `after_sequence` 缺省视为 0；limit 采用与既有 message query 一致的上限策略。
- 反序列化 `payload_json` 失败应作为数据库损坏/Store 错误，不能静默丢弃事件。
- 查询只返回持久化事实；`message.delta` 不应出现在结果中。
- 返回最后 sequence，供 RPC 客户端保存 checkpoint。

测试：

- 同一 session 按 sequence 严格递增。
- 两个 session 的事件互不泄漏。
- after sequence 与 limit 正确分页。
- 手工插入损坏 payload JSON 时读取返回错误。

### 步骤 7：集成与回归测试

新增或扩展：

```text
crates/sagent-store/src/lib.rs          # 端到端 Store 测试
crates/sagent-store/src/migration.rs    # migration 单测
crates/sagent-store/tests/fixtures/historic_v2.sql
```

最小集成场景：

1. 创建会话和 generation。
2. `begin_turn` 写 user 消息与 started event。
3. `commit_tool_result` 写 tool 消息与完成 event。
4. `complete_turn` 写 assistant 消息与 completed event。
5. 关闭 Store，使用新连接重开。
6. 读取 session、display messages、model messages、FTS 与 events_since。
7. 断言恢复内容和 sequence 完整。

Profile 隔离测试使用两个独立临时目录下的绝对 `state.db` 路径：在 A 写入的 turn/event 不得在 B 的 Store 查询到。它测试数据库实例隔离，不把 profile 解析逻辑重复放进 Store。

必须回归的既有行为：

- FTS5 搜索；
- 历史 v1 migration；
- messages active/compacted/display 过滤；
- rewind、restore、retry；
- `Store::open_readonly` 不创建数据库、不迁移、不写 WAL。

### 步骤 8：提交边界与门禁

推荐拆分提交：

1. `feat(types): add persisted turn status and event sequence`
2. `feat(store): migrate schema to v3 for turn event persistence`
3. `feat(store): persist generation and started turns atomically`
4. `feat(store): commit tool and terminal turn outcomes atomically`
5. `feat(store): query persisted daemon events`
6. `test(store): cover v2 migration, rollback and replay isolation`

每个提交都运行：

```text
cargo fmt --check
cargo test -p sagent-types -p sagent-store --offline
cargo clippy -p sagent-types -p sagent-store --all-targets --offline -- -D warnings
```

最后运行：

```text
cargo test --workspace --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
```

## 6. 4.2 验收清单

- [ ] 新建 Sagent 数据库直接得到 schema v3；v1/v2 fixture 均能无损升级。
- [ ] 既有 session/message/FTS/rewind/restore/retry 行为不变。
- [ ] 每个 turn 绑定一个已存在的 generation。
- [ ] begin turn 的 user message、turn row、started event 原子提交。
- [ ] tool 最终结果以普通 tool message 持久化并写入恢复事件。
- [ ] final assistant message、completed turn outcome、完成事件原子提交。
- [ ] interrupted/failed turn 没有伪造 assistant final message。
- [ ] 重复完成不会产生第二条 assistant 消息或第二组完成事件。
- [ ] 可按 `session_id + sequence` 可靠重放持久化事件。
- [ ] Store 仍不依赖 `sagent-agent`、provider、runtime、RPC 或 TUI。
- [ ] workspace 全量测试和 Clippy 通过。

完成 4.2 后，下一步才是 4.3：让 `sagent-runtime::SessionActor` 成为每个 session 的唯一写入者，并调用这些 repository API。

## 步骤 2 执行记录

执行日期：2026-09-02  
状态：已完成

已完成：

- `SCHEMA_VERSION` 从 2 升至 3；
- 新增 `session_generations`、`turns`、`daemon_events` 表及索引；
- 保留现有 `sessions`、`messages`、FTS5 表和触发器；
- v1 数据库可直接升级至 v3；
- v2 数据库可升级至 v3；
- 新增 v2→v3 migration 回归测试。

验证结果：`sagent-store` 40 个测试通过，Clippy（`-D warnings`）通过。
